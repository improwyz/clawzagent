#[cfg(test)]
mod local_tests {
    use std::collections::HashMap;

    use super::super::{LocalBackend, TerminalBackend};

    #[tokio::test]
    async fn local_exec_echo() {
        let dir = std::env::temp_dir().join("clawz-terminal-test");
        let _ = std::fs::create_dir_all(&dir);
        let backend = LocalBackend::new(dir);
        let out = backend
            .exec("echo clawz_terminal", None, 10, &HashMap::new())
            .await
            .unwrap();
        assert!(out.success());
        assert!(out.stdout.contains("clawz_terminal"));
    }

    #[test]
    fn resolve_kind_parse() {
        use super::super::config::{TerminalBackendKind, resolve_backend_kind};
        let _ = resolve_backend_kind;
        assert_eq!(
            TerminalBackendKind::parse("ssh"),
            Some(TerminalBackendKind::Ssh)
        );
    }
}
