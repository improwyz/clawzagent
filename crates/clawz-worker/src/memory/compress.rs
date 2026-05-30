//! Pre-LLM context compression (TokenJuice-style) for standalone deployments.
//!
//! Strips HTML, caps oversized tool outputs, and deduplicates adjacent identical
//! user lines. Uses character-based truncation so CJK text is not split mid-rune.

use clawz_core::types::message::{Message, MessageContent, Role};

/// Configuration for [`compress_messages`].
#[derive(Debug, Clone)]
pub struct CompressConfig {
    /// Maximum characters retained for tool-role messages.
    pub max_tool_output_chars: usize,
    /// Maximum characters retained for assistant text (e.g. long tool summaries).
    pub max_assistant_text_chars: usize,
    /// Strip simple HTML tags before sending to the provider.
    pub strip_html: bool,
    /// Remove consecutive duplicate user messages.
    pub dedupe_adjacent: bool,
}

impl Default for CompressConfig {
    fn default() -> Self {
        Self {
            max_tool_output_chars: 8_000,
            max_assistant_text_chars: 16_000,
            strip_html: true,
            dedupe_adjacent: true,
        }
    }
}

/// Compress messages in place before a provider call.
pub fn compress_messages(messages: &mut Vec<Message>) {
    compress_messages_with_config(messages, &CompressConfig::default());
}

/// Compress with explicit limits.
pub fn compress_messages_with_config(messages: &mut Vec<Message>, config: &CompressConfig) {
    for msg in messages.iter_mut() {
        compress_one_message(msg, config);
    }
    if config.dedupe_adjacent {
        dedupe_adjacent_user_messages(messages);
    }
}

fn compress_one_message(msg: &mut Message, config: &CompressConfig) {
    let max = match msg.role {
        Role::Tool => config.max_tool_output_chars,
        Role::Assistant => config.max_assistant_text_chars,
        _ => return,
    };

    if let MessageContent::Text(text) = &mut msg.content {
        let mut s = if config.strip_html {
            strip_html(text)
        } else {
            text.clone()
        };
        s = truncate_chars(&s, max);
        *text = s;
    }
}

/// Remove simple HTML tags while preserving inner text.
pub fn strip_html(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut in_tag = false;
    for ch in input.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    collapse_whitespace(&out)
}

fn collapse_whitespace(s: &str) -> String {
    let mut out = String::new();
    let mut prev_space = false;
    for ch in s.chars() {
        if ch.is_whitespace() {
            if !prev_space {
                out.push(' ');
                prev_space = true;
            }
        } else {
            out.push(ch);
            prev_space = false;
        }
    }
    out.trim().to_string()
}

/// Truncate at a char boundary with an ellipsis suffix.
pub fn truncate_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let truncated: String = text.chars().take(max_chars).collect();
    format!("{truncated}…")
}

fn dedupe_adjacent_user_messages(messages: &mut Vec<Message>) {
    let mut i = 1usize;
    while i < messages.len() {
        let dup = messages[i].role == Role::User
            && messages[i - 1].role == Role::User
            && message_text(&messages[i]) == message_text(&messages[i - 1]);
        if dup {
            messages.remove(i);
        } else {
            i += 1;
        }
    }
}

fn message_text(msg: &Message) -> Option<String> {
    msg.content.as_text().map(|t| t.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_html_removes_tags() {
        let s = strip_html("<p>Hello <b>world</b></p>");
        assert_eq!(s, "Hello world");
    }

    #[test]
    fn truncate_chars_preserves_cjk() {
        let s = "你好世界测试数据";
        let out = truncate_chars(s, 4);
        assert_eq!(out.chars().count(), 5); // 4 chars + ellipsis
        assert!(out.ends_with('…'));
    }

    #[test]
    fn compress_caps_tool_output() {
        let mut msgs = vec![Message::new(
            Role::Tool,
            MessageContent::text("x".repeat(100)),
        )];
        compress_messages_with_config(
            &mut msgs,
            &CompressConfig {
                max_tool_output_chars: 20,
                max_assistant_text_chars: 20,
                strip_html: false,
                dedupe_adjacent: false,
            },
        );
        let text = msgs[0].content.as_text().unwrap();
        assert!(text.chars().count() <= 21);
    }

    #[test]
    fn dedupe_adjacent_users() {
        let mut msgs = vec![
            Message::user("same"),
            Message::user("same"),
            Message::user("other"),
        ];
        compress_messages_with_config(
            &mut msgs,
            &CompressConfig {
                dedupe_adjacent: true,
                ..Default::default()
            },
        );
        assert_eq!(msgs.len(), 2);
    }
}
