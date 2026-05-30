//! Post-turn skill proposals from conversation transcripts.

use clawz_core::types::message::{Message, Role};

/// Lightweight skill curator — proposes workspace skill patches from transcripts.
pub struct SkillCurator;

impl SkillCurator {
    /// Inspect the transcript and return optional skill markdown to append.
    ///
    /// Personal mode: returns low-risk hints when the user explicitly asks to remember
    /// a workflow. Enterprise callers should route proposals through governance instead.
    pub fn maybe_propose(transcript: &[Message]) -> Option<String> {
        let last_user = transcript
            .iter()
            .rev()
            .find(|m| m.role == Role::User)
            .and_then(|m| m.content.as_text())?;

        let lower = last_user.to_lowercase();
        if lower.contains("remember this workflow")
            || lower.contains("save as skill")
            || lower.contains("add to skills")
        {
            let title = last_user.lines().next().unwrap_or("learned-workflow");
            return Some(format!(
                "# {}\n\nLearned from session on {}.\n\n```\n{}\n```\n",
                title.trim(),
                chrono::Utc::now().format("%Y-%m-%d"),
                last_user.trim()
            ));
        }
        None
    }
}
