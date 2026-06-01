pub mod splash;
pub mod llm_setup;
pub mod chat;

use crossterm::event::Event;
use ratatui::layout::Rect;
use ratatui::Frame;

pub trait Screen {
    fn draw(&self, frame: &mut Frame, area: Rect);
    fn handle_event(&mut self, event: Event) -> Transition;
    fn tick(&mut self) {}
}

pub enum Transition {
    Stay,
    Next(Box<dyn Screen>),
    Quit,
}
