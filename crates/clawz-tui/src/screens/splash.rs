use crossterm::event::{Event, KeyCode, KeyEvent};
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::{Screen, Transition};
use crate::branding;
use crate::screens::llm_setup::LlmSetupScreen;

pub struct SplashScreen {
    ticks: u32,
    host_summary: String,
}

impl SplashScreen {
    pub fn new() -> Self {
        let report = clawz_setup::HostSpecChecker::collect();
        Self {
            ticks: 0,
            host_summary: report.summary,
        }
    }
}

impl Screen for SplashScreen {
    fn draw(&self, frame: &mut Frame, area: Rect) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(1),
                Constraint::Length(9),
                Constraint::Length(1),
                Constraint::Length(3),
                Constraint::Min(1),
                Constraint::Length(1),
            ])
            .split(area);

        let logo_lines: Vec<Line> = branding::LOGO
            .lines()
            .filter(|l| !l.is_empty())
            .map(|l| Line::from(Span::styled(l, Style::default().fg(branding::COPPER))))
            .collect();
        let logo = Paragraph::new(logo_lines).alignment(Alignment::Center);
        frame.render_widget(logo, chunks[1]);

        let tagline = Paragraph::new(Line::from(Span::styled(
            branding::TAGLINE,
            Style::default()
                .fg(branding::SILVER)
                .add_modifier(Modifier::ITALIC),
        )))
        .alignment(Alignment::Center);
        frame.render_widget(tagline, chunks[2]);

        let summary_lines: Vec<Line> = self
            .host_summary
            .lines()
            .map(|l| Line::from(Span::styled(l, Style::default().fg(branding::DIM))))
            .collect();
        let summary = Paragraph::new(summary_lines).alignment(Alignment::Center);
        frame.render_widget(summary, chunks[3]);

        let hint = Paragraph::new(Line::from(Span::styled(
            "Press any key to continue...",
            Style::default().fg(branding::DIM),
        )))
        .alignment(Alignment::Center);
        frame.render_widget(hint, chunks[5]);
    }

    fn handle_event(&mut self, event: Event) -> Transition {
        if let Event::Key(KeyEvent { code, .. }) = event {
            match code {
                KeyCode::Char('q') => return Transition::Quit,
                _ => return Transition::Next(Box::new(LlmSetupScreen::new())),
            }
        }
        Transition::Stay
    }

    fn tick(&mut self) {
        self.ticks += 1;
        if self.ticks >= 20 {
            // Auto-advance not used here — user presses a key
        }
    }
}
