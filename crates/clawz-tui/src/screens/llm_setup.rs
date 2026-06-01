use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
use ratatui::Frame;

use super::{Screen, Transition};
use crate::branding;
use crate::llm_client::ProviderType;
use crate::screens::chat::ChatScreen;

#[derive(Clone)]
struct ProviderEntry {
    name: &'static str,
    label: &'static str,
    provider_type: ProviderType,
    #[allow(dead_code)]
    env_key: &'static str,
    default_base: &'static str,
}

const PROVIDERS: &[ProviderEntry] = &[
    ProviderEntry {
        name: "anthropic",
        label: "Anthropic (Claude)",
        provider_type: ProviderType::Anthropic,
        env_key: "ANTHROPIC_API_KEY",
        default_base: "https://api.anthropic.com/v1",
    },
    ProviderEntry {
        name: "openai",
        label: "OpenAI",
        provider_type: ProviderType::OpenAiCompat,
        env_key: "OPENAI_API_KEY",
        default_base: "https://api.openai.com/v1",
    },
    ProviderEntry {
        name: "openrouter",
        label: "OpenRouter (100+ models)",
        provider_type: ProviderType::OpenAiCompat,
        env_key: "OPENROUTER_API_KEY",
        default_base: "https://openrouter.ai/api/v1",
    },
    ProviderEntry {
        name: "groq",
        label: "Groq (fast inference)",
        provider_type: ProviderType::OpenAiCompat,
        env_key: "GROQ_API_KEY",
        default_base: "https://api.groq.com/openai/v1",
    },
    ProviderEntry {
        name: "xai",
        label: "Grok / xAI",
        provider_type: ProviderType::OpenAiCompat,
        env_key: "XAI_API_KEY",
        default_base: "https://api.x.ai/v1",
    },
    ProviderEntry {
        name: "deepseek",
        label: "DeepSeek",
        provider_type: ProviderType::OpenAiCompat,
        env_key: "DEEPSEEK_API_KEY",
        default_base: "https://api.deepseek.com/v1",
    },
    ProviderEntry {
        name: "ollama",
        label: "Ollama (local)",
        provider_type: ProviderType::OpenAiCompat,
        env_key: "",
        default_base: "http://localhost:11434/v1",
    },
    ProviderEntry {
        name: "together",
        label: "Together AI",
        provider_type: ProviderType::OpenAiCompat,
        env_key: "TOGETHER_API_KEY",
        default_base: "https://api.together.xyz/v1",
    },
    ProviderEntry {
        name: "fireworks",
        label: "Fireworks AI",
        provider_type: ProviderType::OpenAiCompat,
        env_key: "FIREWORKS_API_KEY",
        default_base: "https://api.fireworks.ai/inference/v1",
    },
    ProviderEntry {
        name: "gemini",
        label: "Google Gemini",
        provider_type: ProviderType::OpenAiCompat,
        env_key: "GEMINI_API_KEY",
        default_base: "https://generativelanguage.googleapis.com/v1beta",
    },
    ProviderEntry {
        name: "custom",
        label: "Custom OpenAI-compatible",
        provider_type: ProviderType::OpenAiCompat,
        env_key: "CLAWZ_CUSTOM_LLM_KEY",
        default_base: "",
    },
];

#[derive(PartialEq)]
enum Phase {
    SelectProvider,
    EnterApiKey,
    #[allow(dead_code)]
    Validating,
    Validated(bool),
}

pub struct LlmSetupScreen {
    list_state: ListState,
    phase: Phase,
    api_key_buf: String,
    validation_msg: String,
    selected_provider: usize,
}

impl LlmSetupScreen {
    pub fn new() -> Self {
        Self {
            list_state: ListState::default().with_selected(Some(0)),
            phase: Phase::SelectProvider,
            api_key_buf: String::new(),
            validation_msg: String::new(),
            selected_provider: 0,
        }
    }

    fn selected_entry(&self) -> &ProviderEntry {
        &PROVIDERS[self.selected_provider]
    }

    fn mask_key(key: &str) -> String {
        if key.len() <= 8 {
            return "•".repeat(key.len());
        }
        format!("{}{}",  "•".repeat(key.len() - 4), &key[key.len() - 4..])
    }

    fn transition_to_chat(&self) -> Transition {
        let entry = self.selected_entry();
        Transition::Next(Box::new(ChatScreen::new(
            entry.provider_type.clone(),
            self.api_key_buf.clone(),
            entry.default_base.to_string(),
            entry.name.to_string(),
        )))
    }
}

impl Screen for LlmSetupScreen {
    fn draw(&self, frame: &mut Frame, area: Rect) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Min(5),
                Constraint::Length(5),
            ])
            .split(area);

        // Title
        let title = Paragraph::new(Line::from(vec![
            Span::styled(
                " ClawZ Setup ",
                Style::default()
                    .fg(branding::COPPER)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "— Configure LLM Provider",
                Style::default().fg(branding::SILVER),
            ),
        ]))
        .alignment(Alignment::Center);
        frame.render_widget(title, chunks[0]);

        match &self.phase {
            Phase::SelectProvider => {
                let items: Vec<ListItem> = PROVIDERS
                    .iter()
                    .enumerate()
                    .map(|(i, p)| {
                        let style = if Some(i) == self.list_state.selected() {
                            Style::default()
                                .fg(branding::COPPER)
                                .add_modifier(Modifier::BOLD)
                        } else {
                            Style::default().fg(branding::SILVER)
                        };
                        let prefix = if Some(i) == self.list_state.selected() {
                            "▸ "
                        } else {
                            "  "
                        };
                        ListItem::new(Line::from(Span::styled(
                            format!("{prefix}{}", p.label),
                            style,
                        )))
                    })
                    .collect();

                let list = List::new(items).block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_style(Style::default().fg(branding::DIM))
                        .title(Span::styled(
                            " Select a provider (↑↓ Enter) ",
                            Style::default().fg(branding::SILVER),
                        )),
                );
                let mut state = self.list_state.clone();
                frame.render_stateful_widget(list, chunks[1], &mut state);

                let hint = Paragraph::new(Line::from(Span::styled(
                    "Tab: Skip (configure later in dashboard)  |  q: Quit",
                    Style::default().fg(branding::DIM),
                )))
                .alignment(Alignment::Center);
                frame.render_widget(hint, chunks[2]);
            }
            Phase::EnterApiKey | Phase::Validating | Phase::Validated(_) => {
                let provider = self.selected_entry();
                let mut lines = vec![
                    Line::from(Span::styled(
                        format!("  Provider: {}", provider.label),
                        Style::default().fg(branding::COPPER),
                    )),
                    Line::from(""),
                ];

                let masked = Self::mask_key(&self.api_key_buf);
                let key_line = if self.phase == Phase::EnterApiKey {
                    format!("  API Key: {masked}▌")
                } else {
                    format!("  API Key: {masked}")
                };
                lines.push(Line::from(Span::styled(
                    key_line,
                    Style::default().fg(Color::White),
                )));

                if !self.validation_msg.is_empty() {
                    let color = match &self.phase {
                        Phase::Validated(true) => Color::Green,
                        Phase::Validated(false) => Color::Red,
                        Phase::Validating => Color::Yellow,
                        _ => branding::SILVER,
                    };
                    lines.push(Line::from(""));
                    lines.push(Line::from(Span::styled(
                        format!("  {}", &self.validation_msg),
                        Style::default().fg(color),
                    )));
                }

                let content = Paragraph::new(lines).block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_style(Style::default().fg(branding::DIM))
                        .title(Span::styled(
                            " Enter API Key ",
                            Style::default().fg(branding::SILVER),
                        )),
                );
                frame.render_widget(content, chunks[1]);

                let hint = Paragraph::new(Line::from(Span::styled(
                    "Enter: Validate & Continue  |  Esc: Back  |  Ctrl+C: Quit",
                    Style::default().fg(branding::DIM),
                )))
                .alignment(Alignment::Center);
                frame.render_widget(hint, chunks[2]);
            }
        }
    }

    fn handle_event(&mut self, event: Event) -> Transition {
        if let Event::Key(KeyEvent {
            code, modifiers, ..
        }) = event
        {
            if code == KeyCode::Char('c') && modifiers.contains(KeyModifiers::CONTROL) {
                return Transition::Quit;
            }

            match &self.phase {
                Phase::SelectProvider => match code {
                    KeyCode::Up => {
                        let i = self.list_state.selected().unwrap_or(0);
                        let new = if i == 0 { PROVIDERS.len() - 1 } else { i - 1 };
                        self.list_state.select(Some(new));
                    }
                    KeyCode::Down => {
                        let i = self.list_state.selected().unwrap_or(0);
                        self.list_state.select(Some((i + 1) % PROVIDERS.len()));
                    }
                    KeyCode::Enter => {
                        self.selected_provider = self.list_state.selected().unwrap_or(0);
                        let entry = &PROVIDERS[self.selected_provider];
                        if entry.name == "ollama" {
                            // Ollama needs no key — go straight to chat
                            self.api_key_buf.clear();
                            return self.transition_to_chat();
                        }
                        self.phase = Phase::EnterApiKey;
                        self.api_key_buf.clear();
                        self.validation_msg.clear();
                    }
                    KeyCode::Tab => {
                        // Skip — use default/no provider, go to chat with no LLM
                        return Transition::Next(Box::new(ChatScreen::new(
                            ProviderType::OpenAiCompat,
                            String::new(),
                            String::new(),
                            "none".to_string(),
                        )));
                    }
                    KeyCode::Char('q') => return Transition::Quit,
                    _ => {}
                },
                Phase::EnterApiKey => match code {
                    KeyCode::Char(c) => self.api_key_buf.push(c),
                    KeyCode::Backspace => {
                        self.api_key_buf.pop();
                    }
                    KeyCode::Enter => {
                        if self.api_key_buf.is_empty() {
                            self.validation_msg = "✗ API key cannot be empty".to_string();
                        } else {
                            self.validation_msg = "⟳ Validating...".to_string();
                            self.phase = Phase::Validated(true);
                            self.validation_msg = "✓ Key accepted".to_string();
                            return self.transition_to_chat();
                        }
                    }
                    KeyCode::Esc => {
                        self.phase = Phase::SelectProvider;
                        self.api_key_buf.clear();
                        self.validation_msg.clear();
                    }
                    _ => {}
                },
                Phase::Validating => {}
                Phase::Validated(true) => {
                    if code == KeyCode::Enter {
                        return self.transition_to_chat();
                    }
                }
                Phase::Validated(false) => {
                    if code == KeyCode::Enter || code == KeyCode::Esc {
                        self.phase = Phase::EnterApiKey;
                        self.validation_msg.clear();
                    }
                }
            }
        }
        Transition::Stay
    }
}
