//! The messages a UI shows as toasts or snackbars, kept so one that went by too fast can be
//! read again (Alt+M on the desktop, Settings on the phone).

use std::time::Instant;

/// How many are kept.
const KEPT: usize = 100;

pub struct Message {
    pub text: String,
    pub at: Instant,
    pub error: bool,
}

#[derive(Default)]
pub struct Log {
    /// Oldest first.
    pub list: Vec<Message>,
}

impl Log {
    /// Keeps a message the UI is showing, once (by when it was made).
    pub fn record(&mut self, text: &str, at: Instant, error: bool) {
        if self.list.iter().rev().take(8).any(|m| m.at == at) {
            return;
        }
        self.list.push(Message { text: text.to_string(), at, error });
        if self.list.len() > KEPT {
            self.list.remove(0);
        }
    }
}

/// "now", "40s ago", "3 min ago", "2 h ago".
pub fn ago(at: Instant) -> String {
    let s = at.elapsed().as_secs();
    match s {
        0..=4 => "now".into(),
        5..=59 => format!("{s}s ago"),
        60..=3599 => format!("{} min ago", s / 60),
        _ => format!("{} h ago", s / 3600),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_the_last_hundred_and_each_message_once() {
        let mut log = Log::default();
        let first = Instant::now();
        log.record("one", first, false);
        log.record("one", first, false);
        assert_eq!(log.list.len(), 1);
        for i in 0..150 {
            log.record(&format!("m{i}"), Instant::now() + std::time::Duration::from_nanos(i + 1), true);
        }
        assert_eq!(log.list.len(), KEPT);
        assert_eq!(log.list.last().unwrap().text, "m149");
    }
}
