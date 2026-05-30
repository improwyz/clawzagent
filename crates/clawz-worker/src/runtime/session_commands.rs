//! Chat commands: `/new`, `/reset`, `/compact`, `/usage`.

/// Parsed user input — either a slash command or normal chat text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionCommand {
    New,
    Reset,
    Compact,
    Usage,
    Chat(String),
}

/// Default number of messages kept after `/compact`.
pub const DEFAULT_COMPACT_KEEP: usize = 40;

/// Parse a user message for session control commands (case-sensitive, trimmed).
pub fn parse_command(text: &str) -> SessionCommand {
    let trimmed = text.trim();
    match trimmed {
        "/new" => SessionCommand::New,
        "/reset" => SessionCommand::Reset,
        "/compact" => SessionCommand::Compact,
        "/usage" => SessionCommand::Usage,
        _ => SessionCommand::Chat(trimmed.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_slash_commands() {
        assert_eq!(parse_command("/new"), SessionCommand::New);
        assert_eq!(parse_command("  /reset  "), SessionCommand::Reset);
        assert_eq!(
            parse_command("hello"),
            SessionCommand::Chat("hello".to_string())
        );
    }
}
