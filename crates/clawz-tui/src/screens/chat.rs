use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

use super::{Screen, Transition};
use crate::branding;
use crate::llm_client::{LocalLlmClient, Message, ProviderType};

const SYSTEM_PROMPT: &str = r#"You are ClawZ Setup Assistant. You guide users through deploying and configuring the ClawZ Agent Platform.

Your capabilities:
- Detect the user's system (OS, RAM, Docker availability, ports)
- Recommend deployment mode (standalone, micro, elastic)
- Choose install strategy (prebuilt Docker images, local build, or source)
- Auto-generate secrets (JWT, worker token, encryption key)
- Configure agent identity (name, role, description)
- Execute Docker Compose or cargo build with live progress
- Run health checks on gateway, worker, and database
- Show the dashboard URL when deployment completes

Guide the user step by step. Be concise and friendly. When recommending, explain why briefly.
Start by greeting the user, summarizing what you detected about their system, and recommending a deployment approach."#;

struct ChatMessage {
    role: String,
    content: String,
}

#[allow(dead_code)]
enum ChatMode {
    LocalHelper,
    GatewayAgent,
}

pub struct ChatScreen {
    messages: Vec<ChatMessage>,
    input_buf: String,
    #[allow(dead_code)]
    scroll_offset: u16,
    streaming_buf: Arc<Mutex<String>>,
    is_streaming: bool,
    mode: ChatMode,
    #[allow(dead_code)]
    llm_client: Arc<LocalLlmClient>,
    tx: mpsc::UnboundedSender<ChatCommand>,
    rx: Arc<Mutex<mpsc::UnboundedReceiver<ChatEvent>>>,
    provider_name: String,
}

enum ChatCommand {
    SendMessage(String),
}

enum ChatEvent {
    Chunk(String),
    Done,
    #[allow(dead_code)]
    Error(String),
}

impl ChatScreen {
    pub fn new(
        provider_type: ProviderType,
        api_key: String,
        base_url: String,
        provider_name: String,
    ) -> Self {
        let mut client = LocalLlmClient::new(provider_type, api_key, base_url, &provider_name);
        client.set_system_prompt(SYSTEM_PROMPT.to_string());
        let client = Arc::new(client);

        let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<ChatCommand>();
        let (evt_tx, evt_rx) = mpsc::unbounded_channel::<ChatEvent>();

        let client_clone = client.clone();

        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
            rt.block_on(async move {
                let mut history: Vec<Message> = Vec::new();

                // Send initial greeting
                let (stream_tx, mut stream_rx) = mpsc::unbounded_channel::<String>();
                let client_ref = client_clone.clone();
                let initial_messages = vec![Message {
                    role: "user".to_string(),
                    content: "I just started the ClawZ installer. Please greet me and help me set up.".to_string(),
                }];

                tokio::spawn(async move {
                    let _ = client_ref.send_streaming(&initial_messages, stream_tx).await;
                });

                let mut full_response = String::new();
                while let Some(chunk) = stream_rx.recv().await {
                    full_response.push_str(&chunk);
                    let _ = evt_tx.send(ChatEvent::Chunk(chunk));
                }
                let _ = evt_tx.send(ChatEvent::Done);

                history.push(Message {
                    role: "user".to_string(),
                    content: "I just started the ClawZ installer. Please greet me and help me set up.".to_string(),
                });
                history.push(Message {
                    role: "assistant".to_string(),
                    content: full_response,
                });

                while let Some(cmd) = cmd_rx.recv().await {
                    match cmd {
                        ChatCommand::SendMessage(text) => {
                            history.push(Message {
                                role: "user".to_string(),
                                content: text,
                            });

                            let (stream_tx, mut stream_rx) =
                                mpsc::unbounded_channel::<String>();
                            let client_ref = client_clone.clone();
                            let msgs = history.clone();

                            tokio::spawn(async move {
                                let _ = client_ref.send_streaming(&msgs, stream_tx).await;
                            });

                            let mut full_response = String::new();
                            while let Some(chunk) = stream_rx.recv().await {
                                full_response.push_str(&chunk);
                                let _ = evt_tx.send(ChatEvent::Chunk(chunk));
                            }
                            let _ = evt_tx.send(ChatEvent::Done);

                            history.push(Message {
                                role: "assistant".to_string(),
                                content: full_response,
                            });
                        }
                    }
                }
            });
        });

        Self {
            messages: Vec::new(),
            input_buf: String::new(),
            scroll_offset: 0,
            streaming_buf: Arc::new(Mutex::new(String::new())),
            is_streaming: true, // starts streaming initial greeting
            mode: ChatMode::LocalHelper,
            llm_client: client,
            tx: cmd_tx,
            rx: Arc::new(Mutex::new(evt_rx)),
            provider_name,
        }
    }

    fn process_events(&mut self) {
        if let Ok(mut rx) = self.rx.try_lock() {
            while let Ok(event) = rx.try_recv() {
                match event {
                    ChatEvent::Chunk(chunk) => {
                        if let Ok(mut buf) = self.streaming_buf.try_lock() {
                            buf.push_str(&chunk);
                        }
                    }
                    ChatEvent::Done => {
                        if let Ok(mut buf) = self.streaming_buf.try_lock() {
                            let content = buf.clone();
                            buf.clear();
                            if !content.is_empty() {
                                self.messages.push(ChatMessage {
                                    role: "assistant".to_string(),
                                    content,
                                });
                            }
                        }
                        self.is_streaming = false;
                    }
                    ChatEvent::Error(err) => {
                        self.messages.push(ChatMessage {
                            role: "system".to_string(),
                            content: format!("Error: {err}"),
                        });
                        self.is_streaming = false;
                    }
                }
            }
        }
    }

    fn render_messages(&self) -> Vec<Line> {
        let mut lines = Vec::new();

        for msg in &self.messages {
            let (prefix, color) = match msg.role.as_str() {
                "user" => ("> ", Color::Cyan),
                "assistant" => ("🤖 ", branding::COPPER),
                "system" => ("⚙ ", Color::Yellow),
                _ => ("", branding::SILVER),
            };

            for (i, line) in msg.content.lines().enumerate() {
                let p = if i == 0 { prefix } else { "   " };
                lines.push(Line::from(Span::styled(
                    format!("{p}{line}"),
                    Style::default().fg(color),
                )));
            }
            lines.push(Line::from(""));
        }

        // Streaming buffer
        if let Ok(buf) = self.streaming_buf.try_lock() {
            if !buf.is_empty() {
                for (i, line) in buf.lines().enumerate() {
                    let p = if i == 0 { "🤖 " } else { "   " };
                    lines.push(Line::from(Span::styled(
                        format!("{p}{line}"),
                        Style::default().fg(branding::COPPER),
                    )));
                }
                if self.is_streaming {
                    lines.push(Line::from(Span::styled(
                        "   ▌",
                        Style::default().fg(branding::COPPER),
                    )));
                }
            }
        }

        lines
    }
}

impl Screen for ChatScreen {
    fn draw(&self, frame: &mut Frame, area: Rect) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Min(5),
                Constraint::Length(3),
            ])
            .split(area);

        // Title bar
        let mode_label = match &self.mode {
            ChatMode::LocalHelper => format!("Local Setup Assistant ({})", self.provider_name),
            ChatMode::GatewayAgent => "ClawZ Agent (gateway)".to_string(),
        };
        let title = Paragraph::new(Line::from(vec![
            Span::styled(
                " ClawZ ",
                Style::default()
                    .fg(branding::COPPER)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("— {mode_label}"),
                Style::default().fg(branding::SILVER),
            ),
        ]));
        frame.render_widget(title, chunks[0]);

        // Message area
        let msg_lines = self.render_messages();
        let msg_height = chunks[1].height as usize;
        let total = msg_lines.len();
        let skip = total.saturating_sub(msg_height);
        let visible: Vec<Line> = msg_lines.into_iter().skip(skip).collect();

        let messages = Paragraph::new(visible)
            .wrap(Wrap { trim: false })
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(branding::DIM)),
            );
        frame.render_widget(messages, chunks[1]);

        // Input area
        let input_display = if self.is_streaming {
            "  Waiting for response...".to_string()
        } else {
            format!("  > {}▌", self.input_buf)
        };
        let input = Paragraph::new(Line::from(Span::styled(
            input_display,
            Style::default().fg(Color::White),
        )))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(branding::DIM))
                .title(Span::styled(
                    " Enter ↵ send | Ctrl+C quit ",
                    Style::default().fg(branding::DIM),
                )),
        );
        frame.render_widget(input, chunks[2]);
    }

    fn handle_event(&mut self, event: Event) -> Transition {
        if let Event::Key(KeyEvent {
            code, modifiers, ..
        }) = event
        {
            if code == KeyCode::Char('c') && modifiers.contains(KeyModifiers::CONTROL) {
                return Transition::Quit;
            }

            if self.is_streaming {
                return Transition::Stay;
            }

            match code {
                KeyCode::Char(c) => self.input_buf.push(c),
                KeyCode::Backspace => {
                    self.input_buf.pop();
                }
                KeyCode::Enter => {
                    let text = self.input_buf.trim().to_string();
                    if !text.is_empty() {
                        self.messages.push(ChatMessage {
                            role: "user".to_string(),
                            content: text.clone(),
                        });
                        self.input_buf.clear();
                        self.is_streaming = true;
                        let _ = self.tx.send(ChatCommand::SendMessage(text));
                    }
                }
                _ => {}
            }
        }
        Transition::Stay
    }

    fn tick(&mut self) {
        self.process_events();
    }
}
