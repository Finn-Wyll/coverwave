use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::VecDeque;
use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Art,
    Library,
}

impl Mode {
    pub fn next(self) -> Mode {
        match self {
            Mode::Art => Mode::Library,
            Mode::Library => Mode::Art,
        }
    }
}

pub struct App {
    pub mode: Mode,
    pub fps: u32,
    pub frame: u32,
    pub show_debug: bool,
    pub should_quit: bool,
    /// Seed for the idle scene. Fixed so the idle animation is the same every
    /// launch; real covers derive theirs from the image hash (M4).
    pub idle_seed: u64,
    /// Percentage of art cells that changed on the last frame (debug overlay).
    pub last_change_pct: f32,
    /// Previous art frame, used to measure how many cells changed.
    pub prev_art: Option<cw_core::CellGrid>,
    frame_times: VecDeque<Instant>,
}

impl App {
    pub fn new(fps: u32) -> Self {
        App {
            mode: Mode::Art,
            fps,
            frame: 0,
            show_debug: false,
            should_quit: false,
            idle_seed: 0x00C0_7E2A_7A7E,
            last_change_pct: 0.0,
            prev_art: None,
            frame_times: VecDeque::with_capacity(64),
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => self.should_quit = true,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.should_quit = true
            }
            KeyCode::Tab => self.mode = self.mode.next(),
            KeyCode::Char('D') => self.show_debug = !self.show_debug,
            _ => {}
        }
    }

    pub fn on_frame_drawn(&mut self, at: Instant) {
        self.frame = self.frame.wrapping_add(1);
        if self.frame_times.len() == 60 {
            self.frame_times.pop_front();
        }
        self.frame_times.push_back(at);
    }

    /// Measured frames per second over the last ~2 seconds.
    pub fn measured_fps(&self) -> f32 {
        match (self.frame_times.front(), self.frame_times.back()) {
            (Some(first), Some(last)) if self.frame_times.len() > 1 => {
                let secs = last.duration_since(*first).as_secs_f32();
                if secs > 0.0 {
                    (self.frame_times.len() - 1) as f32 / secs
                } else {
                    0.0
                }
            }
            _ => 0.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::KeyEvent;

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn tab_cycles_modes_and_q_quits() {
        let mut app = App::new(30);
        app.on_key(press(KeyCode::Tab));
        assert_eq!(app.mode, Mode::Library);
        app.on_key(press(KeyCode::Tab));
        assert_eq!(app.mode, Mode::Art);
        app.on_key(press(KeyCode::Char('D')));
        assert!(app.show_debug);
        app.on_key(press(KeyCode::Char('q')));
        assert!(app.should_quit);
    }
}
