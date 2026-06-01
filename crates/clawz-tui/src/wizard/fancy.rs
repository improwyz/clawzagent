//! Ratatui setup wizard (TTY, `CLAWZ_TUI` not `plain`).

use clawz_setup::{
    DeploymentChoice, HostSpecChecker, InstallStrategy, Result, SetupEvent, SetupStateMachine,
    SetupStep,
};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::ExecutableCommand;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Frame;
use std::io::stdout;
use std::time::Duration;

use super::context::WizardAnswers;
use super::{deployment_label, ensure_secrets, install_label};

struct WizardApp {
    host_summary: String,
    host_warnings: Vec<String>,
    deploy_list: ListState,
    install_list: ListState,
    input_buf: String,
    input_label: &'static str,
    input_secret: bool,
    phase: InputPhase,
    running: bool,
    status: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InputPhase {
    None,
    JwtSecret,
    ApiKeys,
    WorkerToken,
    AnthropicKey,
    OpenaiKey,
    AgentName,
    AgentWho,
    AgentRole,
}

impl WizardApp {
    fn new() -> Self {
        let report = HostSpecChecker::collect();
        Self {
            host_summary: report.summary,
            host_warnings: report.warnings,
            deploy_list: ListState::default().with_selected(Some(1)),
            install_list: ListState::default().with_selected(Some(0)),
            input_buf: String::new(),
            input_label: "",
            input_secret: false,
            phase: InputPhase::None,
            running: true,
            status: String::new(),
        }
    }

    fn step_title(step: SetupStep) -> &'static str {
        match step {
            SetupStep::Welcome => "Welcome",
            SetupStep::DeployMode => "Deployment mode",
            SetupStep::InstallStrategy => "Install strategy",
            SetupStep::Stack => "Stack",
            SetupStep::WriteSecrets => "Secrets",
            SetupStep::Llm => "LLM keys",
            SetupStep::AgentIdentity => "Agent identity",
            SetupStep::Skills => "Skills",
            SetupStep::AgentTopology => "Topology",
            SetupStep::Verify => "Verify",
            SetupStep::Complete => "Complete",
        }
    }

    fn handle_key(
        &mut self,
        key: KeyEvent,
        sm: &mut SetupStateMachine,
        answers: &mut WizardAnswers,
    ) -> Result<bool> {
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            sm.abort(Some("Ctrl+C".into()))?;
            self.running = false;
            return Ok(true);
        }

        if self.phase != InputPhase::None {
            return self.handle_input_key(key, sm, answers);
        }

        match sm.current_step() {
            SetupStep::Welcome
            | SetupStep::Stack
            | SetupStep::Skills
            | SetupStep::AgentTopology
            | SetupStep::Verify
            | SetupStep::Complete => self.handle_nav_key(key, sm, answers),
            SetupStep::DeployMode => self.handle_list_key(key, sm, answers, true),
            SetupStep::InstallStrategy => self.handle_list_key(key, sm, answers, false),
            SetupStep::WriteSecrets => self.handle_secrets_key(key, sm, answers),
            SetupStep::Llm => self.handle_llm_key(key, sm, answers),
            SetupStep::AgentIdentity => self.handle_identity_key(key, sm, answers),
        }
    }

    fn handle_nav_key(
        &mut self,
        key: KeyEvent,
        sm: &mut SetupStateMachine,
        _answers: &mut WizardAnswers,
    ) -> Result<bool> {
        match key.code {
            KeyCode::Enter | KeyCode::Char(' ') => {
                if sm.current_step() == SetupStep::Complete {
                    self.running = false;
                } else {
                    sm.advance()?;
                }
            }
            KeyCode::Esc => {
                sm.abort(Some("user quit".into()))?;
                self.running = false;
            }
            _ => {}
        }
        Ok(false)
    }

    fn handle_list_key(
        &mut self,
        key: KeyEvent,
        sm: &mut SetupStateMachine,
        answers: &mut WizardAnswers,
        deploy: bool,
    ) -> Result<bool> {
        let state = if deploy {
            &mut self.deploy_list
        } else {
            &mut self.install_list
        };
        let len = 3;
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                let i = state.selected().unwrap_or(0);
                state.select(Some(i.saturating_sub(1)));
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let i = state.selected().unwrap_or(0);
                state.select(Some((i + 1).min(len - 1)));
            }
            KeyCode::Enter => {
                let idx = state.selected().unwrap_or(0);
                if deploy {
                    let choice = match idx {
                        0 => DeploymentChoice::Standalone,
                        1 => DeploymentChoice::Micro,
                        _ => DeploymentChoice::Elastic,
                    };
                    sm.set_deployment(choice)?;
                    record_answer(
                        sm,
                        SetupStep::DeployMode,
                        "deployment",
                        deployment_label(choice),
                    );
                    std::env::set_var("CLAWZ_MODE", deployment_label(choice));
                    sm.advance()?;
                } else {
                    let strategy = match idx {
                        0 => InstallStrategy::Prebuilt,
                        1 => InstallStrategy::Build,
                        _ => InstallStrategy::Source,
                    };
                    sm.set_install_strategy(strategy)?;
                    record_answer(
                        sm,
                        SetupStep::InstallStrategy,
                        "install_strategy",
                        install_label(strategy),
                    );
                    sm.advance()?;
                }
                let _ = answers;
            }
            KeyCode::Esc => {
                sm.abort(Some("user quit".into()))?;
                self.running = false;
            }
            _ => {}
        }
        Ok(false)
    }

    fn handle_secrets_key(
        &mut self,
        key: KeyEvent,
        sm: &mut SetupStateMachine,
        answers: &mut WizardAnswers,
    ) -> Result<bool> {
        match key.code {
            KeyCode::Char('1') => self.start_input(InputPhase::JwtSecret, "JWT secret", true),
            KeyCode::Char('2') => self.start_input(InputPhase::ApiKeys, "API keys", false),
            KeyCode::Char('3') => self.start_input(InputPhase::WorkerToken, "Worker token", true),
            KeyCode::Enter => {
                ensure_secrets(answers);
                sm.advance()?;
            }
            KeyCode::Esc => {
                sm.abort(Some("user quit".into()))?;
                self.running = false;
            }
            _ => {}
        }
        Ok(false)
    }

    fn handle_llm_key(
        &mut self,
        key: KeyEvent,
        sm: &mut SetupStateMachine,
        _answers: &mut WizardAnswers,
    ) -> Result<bool> {
        match key.code {
            KeyCode::Char('1') => self.start_input(InputPhase::AnthropicKey, "Anthropic key", true),
            KeyCode::Char('2') => self.start_input(InputPhase::OpenaiKey, "OpenAI key", true),
            KeyCode::Enter => {
                let _ = sm.advance()?;
            }
            KeyCode::Esc => {
                sm.abort(Some("user quit".into()))?;
                self.running = false;
            }
            _ => {}
        }
        Ok(false)
    }

    fn handle_identity_key(
        &mut self,
        key: KeyEvent,
        sm: &mut SetupStateMachine,
        _answers: &mut WizardAnswers,
    ) -> Result<bool> {
        match key.code {
            KeyCode::Char('1') => self.start_input(InputPhase::AgentName, "Agent name", false),
            KeyCode::Char('2') => self.start_input(InputPhase::AgentWho, "Who am I?", false),
            KeyCode::Char('3') => self.start_input(InputPhase::AgentRole, "Role", false),
            KeyCode::Enter => {
                sm.advance()?;
            }
            KeyCode::Esc => {
                sm.abort(Some("user quit".into()))?;
                self.running = false;
            }
            _ => {}
        }
        Ok(false)
    }

    fn start_input(&mut self, phase: InputPhase, label: &'static str, secret: bool) {
        self.phase = phase;
        self.input_label = label;
        self.input_secret = secret;
        self.input_buf.clear();
        self.status = format!("Editing {label} — Enter save, Esc cancel");
    }

    fn handle_input_key(
        &mut self,
        key: KeyEvent,
        sm: &mut SetupStateMachine,
        answers: &mut WizardAnswers,
    ) -> Result<bool> {
        match key.code {
            KeyCode::Esc => {
                self.phase = InputPhase::None;
                self.status.clear();
            }
            KeyCode::Enter => {
                let value = self.input_buf.trim().to_string();
                match self.phase {
                    InputPhase::JwtSecret => answers.jwt_secret = value,
                    InputPhase::ApiKeys => answers.api_keys = value,
                    InputPhase::WorkerToken => answers.worker_token = value,
                    InputPhase::AnthropicKey => answers.anthropic_key = value,
                    InputPhase::OpenaiKey => answers.openai_key = value,
                    InputPhase::AgentName => {
                        answers.agent_name = value;
                        record_answer(sm, SetupStep::AgentIdentity, "name", &answers.agent_name);
                    }
                    InputPhase::AgentWho => answers.agent_who = value,
                    InputPhase::AgentRole => answers.agent_role = value,
                    InputPhase::None => {}
                }
                self.phase = InputPhase::None;
                self.input_buf.clear();
                self.status = "Saved.".into();
            }
            KeyCode::Backspace => {
                self.input_buf.pop();
            }
            KeyCode::Char(c) => self.input_buf.push(c),
            _ => {}
        }
        Ok(false)
    }
}

pub fn run(sm: &mut SetupStateMachine, answers: &mut WizardAnswers) -> Result<()> {
    answers.port = "3000".into();
    answers.log_level = "info".into();
    answers.agent_name = "clawz-assistant".into();
    answers.agent_who = "ClawZ setup assistant".into();
    answers.agent_role = "help users install and operate ClawZ".into();

    let mut stdout = stdout();
    enable_raw_mode()?;
    stdout.execute(EnterAlternateScreen)?;
    let mut terminal = ratatui::Terminal::new(ratatui::backend::CrosstermBackend::new(stdout))?;

    let mut app = WizardApp::new();
    let mut aborted = false;

    while app.running {
        terminal.draw(|f| draw_ui(f, sm, &mut app, answers))?;

        if sm.current_step() == SetupStep::Complete {
            if event::poll(Duration::from_millis(100))? {
                if let Event::Key(key) = event::read()? {
                    if key.code == KeyCode::Enter || key.code == KeyCode::Char(' ') {
                        app.running = false;
                    } else if key.code == KeyCode::Esc {
                        aborted = true;
                        app.running = false;
                    }
                }
            }
            continue;
        }

        if event::poll(Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                if app.handle_key(key, sm, answers)? {
                    aborted = true;
                    break;
                }
            }
        }
    }

    if !aborted && !sm.session().aborted {
        sm.complete()?;
    }

    disable_raw_mode()?;
    terminal.backend_mut().execute(LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}

fn draw_ui(
    f: &mut Frame,
    sm: &SetupStateMachine,
    app: &mut WizardApp,
    answers: &mut WizardAnswers,
) {
    let step = sm.current_step();
    let phase = step.phase();

    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(8),
            Constraint::Length(3),
        ])
        .split(f.area());

    let progress = Paragraph::new(Line::from(vec![
        Span::styled(
            " ClawZ Setup ",
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!(
            "  step {phase}/10 — {}",
            WizardApp::step_title(step)
        )),
    ]))
    .block(Block::default().borders(Borders::ALL).title("Progress"));
    f.render_widget(progress, outer[0]);

    let body = Block::default()
        .borders(Borders::ALL)
        .title(WizardApp::step_title(step));
    let inner = body.inner(outer[1]);
    f.render_widget(body, outer[1]);

    match step {
        SetupStep::Welcome => draw_welcome(f, inner, app),
        SetupStep::DeployMode => draw_deploy_list(f, inner, &mut app.deploy_list),
        SetupStep::InstallStrategy => draw_install_list(f, inner, &mut app.install_list),
        SetupStep::Stack => draw_hint(
            f,
            inner,
            "Stack dependencies install on apply.\n\nEnter — continue",
        ),
        SetupStep::WriteSecrets => draw_secrets(f, inner, answers, app),
        SetupStep::Llm => draw_llm(f, inner, answers, app),
        SetupStep::AgentIdentity => draw_identity(f, inner, answers, app),
        SetupStep::Skills => draw_hint(
            f,
            inner,
            "Skills can be added later under ~/.clawz/workspace/skills/\n\nEnter — continue",
        ),
        SetupStep::AgentTopology => draw_hint(
            f,
            inner,
            "Agent topology follows your deployment mode.\n\nEnter — continue",
        ),
        SetupStep::Verify => draw_verify(f, inner, app),
        SetupStep::Complete => draw_complete(f, inner),
    }

    let help = if app.phase != InputPhase::None {
        format!(
            "{}: {}",
            app.input_label,
            if app.input_secret {
                mask(&app.input_buf)
            } else {
                app.input_buf.clone()
            }
        )
    } else if !app.status.is_empty() {
        app.status.clone()
    } else {
        footer_for_step(step)
    };
    let footer = Paragraph::new(help)
        .wrap(Wrap { trim: true })
        .block(Block::default().borders(Borders::ALL).title("Keys"));
    f.render_widget(footer, outer[2]);
}

fn footer_for_step(step: SetupStep) -> String {
    match step {
        SetupStep::DeployMode | SetupStep::InstallStrategy => {
            "↑/↓ select · Enter confirm · Esc quit".into()
        }
        SetupStep::WriteSecrets => {
            "1 JWT · 2 API keys · 3 worker token · Enter continue · Esc quit".into()
        }
        SetupStep::Llm => "1 Anthropic · 2 OpenAI · Enter continue · Esc quit".into(),
        SetupStep::AgentIdentity => "1 name · 2 who · 3 role · Enter continue · Esc quit".into(),
        _ => "Enter continue · Esc quit · Ctrl+C abort".into(),
    }
}

fn draw_welcome(f: &mut Frame, area: Rect, app: &WizardApp) {
    let mut lines = vec![Line::from("Host specification:"), Line::from("")];
    for l in app.host_summary.lines() {
        lines.push(Line::from(l));
    }
    if !app.host_warnings.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::styled(
            "Warnings:",
            Style::default().fg(Color::Yellow),
        ));
        for w in &app.host_warnings {
            lines.push(Line::from(format!("  • {w}")));
        }
    }
    lines.push(Line::from(""));
    lines.push(Line::styled(
        "Press Enter to continue",
        Style::default().fg(Color::Cyan),
    ));
    let p = Paragraph::new(lines).wrap(Wrap { trim: true });
    f.render_widget(p, area);
}

fn draw_deploy_list(f: &mut Frame, area: Rect, state: &mut ListState) {
    let items = vec![
        ListItem::new("standalone — single binary"),
        ListItem::new("micro      — Docker Compose (recommended)"),
        ListItem::new("elastic    — mesh + leader election"),
    ];
    let list = List::new(items)
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .highlight_symbol("▸ ");
    f.render_stateful_widget(list, area, state);
}

fn draw_install_list(f: &mut Frame, area: Rect, state: &mut ListState) {
    let items = vec![
        ListItem::new("prebuilt — pull GHCR images"),
        ListItem::new("build    — local docker compose build"),
        ListItem::new("source   — cargo, no Docker"),
    ];
    let list = List::new(items)
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .highlight_symbol("▸ ");
    f.render_stateful_widget(list, area, state);
}

fn draw_secrets(f: &mut Frame, area: Rect, answers: &mut WizardAnswers, app: &WizardApp) {
    ensure_secrets(answers);
    let text = format!(
        "Secrets summary (masked):\n\n  JWT:    {}\n  API:    {}\n  Worker: {}\n\n1/2/3 edit fields · Enter when done",
        mask(&answers.jwt_secret),
        mask(&answers.api_keys),
        mask(&answers.worker_token),
    );
    let p = Paragraph::new(text).wrap(Wrap { trim: true });
    f.render_widget(p, area);
    let _ = app;
}

fn draw_llm(f: &mut Frame, area: Rect, answers: &WizardAnswers, _app: &WizardApp) {
    let text = format!(
        "LLM API keys (optional):\n\n  Anthropic: {}\n  OpenAI:    {}\n\n1/2 to edit · Enter continue",
        mask(&answers.anthropic_key),
        mask(&answers.openai_key),
    );
    f.render_widget(Paragraph::new(text).wrap(Wrap { trim: true }), area);
}

fn draw_identity(f: &mut Frame, area: Rect, answers: &WizardAnswers, _app: &WizardApp) {
    let text = format!(
        "Agent identity:\n\n  Name: {}\n  Who:  {}\n  Role: {}\n\n1/2/3 to edit · Enter continue",
        answers.agent_name, answers.agent_who, answers.agent_role,
    );
    f.render_widget(Paragraph::new(text).wrap(Wrap { trim: true }), area);
}

fn draw_verify(f: &mut Frame, area: Rect, app: &WizardApp) {
    let mut lines = vec![Line::from("Verification:"), Line::from("")];
    if app.host_warnings.is_empty() {
        lines.push(Line::styled(
            "All host checks passed.",
            Style::default().fg(Color::Green),
        ));
    } else {
        lines.push(Line::styled(
            "Warnings:",
            Style::default().fg(Color::Yellow),
        ));
        for w in &app.host_warnings {
            lines.push(Line::from(format!("  • {w}")));
        }
    }
    lines.push(Line::from(""));
    lines.push(Line::styled(
        "Enter — finish setup",
        Style::default().fg(Color::Cyan),
    ));
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), area);
}

fn draw_complete(f: &mut Frame, area: Rect) {
    let text = Text::from(vec![
        Line::styled(
            "Setup complete!",
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ),
        Line::from(""),
        Line::from("Export lines will be printed after you exit the wizard."),
        Line::from(""),
        Line::styled("Press Enter to exit", Style::default().fg(Color::Cyan)),
    ]);
    let p = Paragraph::new(text)
        .alignment(Alignment::Center)
        .wrap(Wrap { trim: true });
    f.render_widget(p, area);
}

fn draw_hint(f: &mut Frame, area: Rect, text: &str) {
    f.render_widget(Paragraph::new(text).wrap(Wrap { trim: true }), area);
}

fn mask(s: &str) -> String {
    if s.is_empty() {
        return "(not set)".into();
    }
    crate::mask_key(s)
}

fn record_answer(sm: &mut SetupStateMachine, step: SetupStep, field: &str, value: &str) {
    sm.session_mut().events.push(SetupEvent::UserAnswer {
        step,
        field: field.into(),
        value: value.into(),
    });
}
