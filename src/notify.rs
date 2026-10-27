use std::time::{Duration, Instant};

pub struct Notification {
    pub title: String,
    pub body: String,
    pub created: Instant,
    pub timeout: Duration,
    pub dismissed: bool,
    pub created_at: chrono::DateTime<chrono::Local>,
}

impl Notification {
    pub fn is_popup(&self) -> bool {
        !self.dismissed && self.created.elapsed() < self.timeout
    }
}

#[derive(Default)]
pub struct Notifications {
    /// Oldest first.
    pub items: Vec<Notification>,
    /// Number of notifications not yet seen in the history view.
    pub unread: usize,
}

const HISTORY: usize = 50;

impl Notifications {
    pub fn push(&mut self, title: impl Into<String>, body: impl Into<String>, timeout: Duration) {
        self.items.push(Notification {
            title: title.into(),
            body: body.into(),
            created: Instant::now(),
            timeout,
            dismissed: false,
            created_at: chrono::Local::now(),
        });
        self.unread += 1;
        if self.items.len() > HISTORY {
            self.items.remove(0);
        }
    }

    /// Popups currently on screen, newest first.
    pub fn popups(&self) -> impl Iterator<Item = (usize, &Notification)> {
        self.items.iter().enumerate().rev().filter(|(_, n)| n.is_popup()).take(4)
    }

    pub fn has_popups(&self) -> bool {
        self.items.iter().any(Notification::is_popup)
    }

    pub fn dismiss(&mut self, idx: usize) {
        if let Some(n) = self.items.get_mut(idx) {
            n.dismissed = true;
        }
    }

    pub fn clear(&mut self) {
        self.items.clear();
        self.unread = 0;
    }
}
