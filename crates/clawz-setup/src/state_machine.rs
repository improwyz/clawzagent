//! Setup wizard state machine (phases 0–10).

use crate::error::{Result, SetupError};
use crate::session::{load_session_from, session_to_json};
use crate::types::{
    DeploymentChoice, InstallStrategy, SetupEvent, SetupPlatform, SetupSession, SetupStep,
};

/// Drives step transitions and session persistence.
#[derive(Debug, Clone)]
pub struct SetupStateMachine {
    session: SetupSession,
    persist: bool,
}

impl SetupStateMachine {
    pub fn new(platform: SetupPlatform) -> Self {
        Self {
            session: SetupSession::new(platform),
            persist: true,
        }
    }

    pub fn from_session(session: SetupSession) -> Self {
        Self {
            session,
            persist: true,
        }
    }

    /// Load existing session from disk; returns `None` if missing.
    pub fn resume(_platform: SetupPlatform) -> Result<Option<Self>> {
        match crate::session::load_session() {
            Ok(session) => Ok(Some(Self::from_session(session))),
            Err(SetupError::SessionNotFound { .. }) => Ok(None),
            Err(e) => Err(e),
        }
    }

    pub fn resume_from(path: &std::path::Path) -> Result<Option<Self>> {
        match load_session_from(path) {
            Ok(session) => Ok(Some(Self::from_session(session))),
            Err(SetupError::SessionNotFound { .. }) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Disable automatic persistence (useful in tests).
    pub fn without_persistence(mut self) -> Self {
        self.persist = false;
        self
    }

    pub fn session(&self) -> &SetupSession {
        &self.session
    }

    pub fn session_mut(&mut self) -> &mut SetupSession {
        &mut self.session
    }

    pub fn current_step(&self) -> SetupStep {
        self.session.current_step
    }

    pub fn set_deployment(&mut self, choice: DeploymentChoice) -> Result<()> {
        self.ensure_not_aborted()?;
        self.session.deployment = Some(choice);
        self.session
            .events
            .push(SetupEvent::DeploymentChosen { choice });
        self.session.touch();
        self.persist_if_enabled()
    }

    pub fn set_install_strategy(&mut self, strategy: InstallStrategy) -> Result<()> {
        self.ensure_not_aborted()?;
        self.session.install_strategy = Some(strategy);
        self.session
            .events
            .push(SetupEvent::InstallStrategyChosen { strategy });
        self.session.touch();
        self.persist_if_enabled()
    }

    pub fn advance(&mut self) -> Result<SetupStep> {
        self.ensure_not_aborted()?;
        let current = self.session.current_step;
        let next = current
            .next()
            .ok_or_else(|| SetupError::InvalidTransition("already at final step".into()))?;
        self.go_to(next)
    }

    pub fn go_to(&mut self, step: SetupStep) -> Result<SetupStep> {
        self.ensure_not_aborted()?;
        validate_transition(&self.session, step)?;
        self.session.current_step = step;
        self.session.events.push(SetupEvent::StepEntered { step });
        self.session.touch();
        self.persist_if_enabled()?;
        Ok(step)
    }

    pub fn reset(&mut self) -> Result<()> {
        let platform = self.session.platform;
        self.session = SetupSession::new(platform);
        self.persist_if_enabled()
    }

    pub fn abort(&mut self, reason: Option<String>) -> Result<()> {
        self.session.aborted = true;
        self.session.events.push(SetupEvent::Aborted { reason });
        self.session.touch();
        self.persist_if_enabled()
    }

    pub fn complete(&mut self) -> Result<()> {
        self.go_to(SetupStep::Complete)?;
        self.session.events.push(SetupEvent::Completed);
        self.session.touch();
        self.persist_if_enabled()?;
        let mut user = crate::config::ClawzUserConfig::load()?;
        user.mark_setup_complete()?;
        Ok(())
    }

    pub fn export_json(&self) -> Result<String> {
        session_to_json(&self.session)
    }

    fn ensure_not_aborted(&self) -> Result<()> {
        if self.session.aborted {
            return Err(SetupError::Aborted);
        }
        Ok(())
    }

    fn persist_if_enabled(&self) -> Result<()> {
        if self.persist {
            crate::session::save_session(&self.session)?;
        }
        Ok(())
    }
}

fn validate_transition(session: &SetupSession, target: SetupStep) -> Result<()> {
    if target.requires_deploy_mode() && !session.deploy_mode_chosen() {
        return Err(SetupError::DeployModeRequired {
            step: format!("{target:?}"),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ClawzUserConfig;
    use crate::session::save_session_to;
    use tempfile::TempDir;

    fn machine(platform: SetupPlatform) -> SetupStateMachine {
        SetupStateMachine::new(platform).without_persistence()
    }

    #[test]
    fn advance_welcome_to_deploy_mode() {
        let mut sm = machine(SetupPlatform::Linux);
        assert_eq!(sm.current_step(), SetupStep::Welcome);
        sm.advance().expect("advance");
        assert_eq!(sm.current_step(), SetupStep::DeployMode);
    }

    #[test]
    fn cannot_skip_to_write_secrets_without_deploy_mode() {
        let mut sm = machine(SetupPlatform::Linux);
        let err = sm.go_to(SetupStep::WriteSecrets).unwrap_err();
        assert!(matches!(err, SetupError::DeployModeRequired { .. }));
    }

    #[test]
    fn can_reach_write_secrets_after_deploy_mode() {
        let mut sm = machine(SetupPlatform::Linux);
        sm.set_deployment(DeploymentChoice::Standalone)
            .expect("deploy");
        sm.go_to(SetupStep::WriteSecrets).expect("secrets step");
        assert_eq!(sm.current_step(), SetupStep::WriteSecrets);
    }

    #[test]
    fn advance_blocked_past_stack_without_deploy_mode() {
        let mut sm = machine(SetupPlatform::Linux);
        sm.go_to(SetupStep::Stack).expect("stack");
        let err = sm.advance().unwrap_err();
        assert!(matches!(err, SetupError::DeployModeRequired { .. }));
    }

    #[test]
    fn reset_clears_session() {
        let mut sm = machine(SetupPlatform::Web);
        sm.set_deployment(DeploymentChoice::Micro).expect("deploy");
        sm.go_to(SetupStep::Llm).expect("llm");
        sm.reset().expect("reset");
        assert_eq!(sm.current_step(), SetupStep::Welcome);
        assert!(sm.session().deployment.is_none());
    }

    #[test]
    fn abort_marks_session() {
        let mut sm = machine(SetupPlatform::Linux);
        sm.abort(Some("user quit".into())).expect("abort");
        assert!(sm.session().aborted);
        assert!(sm.advance().is_err());
    }

    #[test]
    fn session_persist_and_resume() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("session.json");

        let mut sm = SetupStateMachine::new(SetupPlatform::Linux).without_persistence();
        sm.set_deployment(DeploymentChoice::Elastic)
            .expect("deploy");
        sm.go_to(SetupStep::InstallStrategy).expect("step");
        save_session_to(sm.session(), &path).expect("save");

        let resumed = SetupStateMachine::resume_from(&path)
            .expect("resume")
            .expect("some");
        assert_eq!(resumed.current_step(), SetupStep::InstallStrategy);
        assert_eq!(
            resumed.session().deployment,
            Some(DeploymentChoice::Elastic)
        );
    }

    #[test]
    fn user_config_setup_complete() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("config.json");
        let mut cfg = ClawzUserConfig::default();
        cfg.setup_complete = true;
        cfg.save_to(&path).expect("save");
        let loaded = ClawzUserConfig::load_from(&path).expect("load");
        assert!(loaded.setup_complete);
    }

    #[test]
    fn export_json_contains_step() {
        let sm = machine(SetupPlatform::MacOs);
        let json = sm.export_json().expect("json");
        assert!(json.contains("\"current_step\""));
        assert!(json.contains("welcome"));
    }
}
