//! Session persistence (`~/.clawz/setup/session.json`).

use std::fs;
use std::path::Path;

use crate::error::{Result, SetupError};
use crate::paths::{session_path, setup_dir};
use crate::types::SetupSession;

pub fn save_session(session: &SetupSession) -> Result<()> {
    save_session_to(session, &session_path())
}

pub fn save_session_to(session: &SetupSession, path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let data = session_to_json(session)?;
    fs::write(path, data)?;
    Ok(())
}

pub fn load_session() -> Result<SetupSession> {
    load_session_from(&session_path())
}

pub fn load_session_from(path: &Path) -> Result<SetupSession> {
    if !path.exists() {
        return Err(SetupError::SessionNotFound {
            path: path.display().to_string(),
        });
    }
    let data = fs::read_to_string(path)?;
    serde_json::from_str(&data).map_err(|e| SetupError::serialization(e.to_string()))
}

/// Serialize session for CLI / web hosts.
pub fn session_to_json(session: &SetupSession) -> Result<String> {
    serde_json::to_string_pretty(session).map_err(|e| SetupError::serialization(e.to_string()))
}

pub fn ensure_setup_dir() -> Result<()> {
    fs::create_dir_all(setup_dir())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{DeploymentChoice, SetupPlatform};
    use tempfile::TempDir;

    #[test]
    fn session_round_trip() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("session.json");

        let mut session = SetupSession::new(SetupPlatform::Linux);
        session.deployment = Some(DeploymentChoice::Micro);
        save_session_to(&session, &path).expect("save");
        let loaded = load_session_from(&path).expect("load");
        assert_eq!(loaded.id, session.id);
        assert_eq!(loaded.deployment, Some(DeploymentChoice::Micro));
    }
}
