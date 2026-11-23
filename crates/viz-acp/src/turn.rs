//! Pure turn control. The async driver supplies clock readings and tool state.

use std::time::{Duration, Instant};

/// Updates can share one publish until this delay expires.
const BATCH: Duration = Duration::from_millis(75);
/// A cancelled prompt must settle before the session can be reused.
const CANCEL_GRACE: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Copy)]
pub enum Phase {
    Running,
    Steering,
    Cancelling { deadline: Instant },
}

#[derive(Debug, Clone, Copy)]
pub enum Signal {
    Update,
    BatchReady,
    Steer,
    Cancel,
    CancelExpired,
}

#[derive(Debug, Clone, Copy)]
pub enum Action {
    Wait,
    Publish,
    Cancel,
    Timeout,
}

#[derive(Debug)]
pub struct Turn {
    phase: Phase,
    batch: Option<Instant>,
}

impl Turn {
    pub fn new(now: Instant, has_updates: bool) -> Self {
        Self {
            phase: Phase::Running,
            batch: has_updates.then(|| now + BATCH),
        }
    }

    pub const fn batch_deadline(&self) -> Option<Instant> {
        self.batch
    }

    pub const fn cancel_deadline(&self) -> Option<Instant> {
        match self.phase {
            Phase::Cancelling { deadline } => Some(deadline),
            Phase::Running | Phase::Steering => None,
        }
    }

    pub const fn accepts_steer(&self) -> bool {
        matches!(self.phase, Phase::Running)
    }

    pub const fn cancelling(&self) -> bool {
        matches!(self.phase, Phase::Cancelling { .. })
    }

    pub fn advance(&mut self, signal: Signal, now: Instant) -> Action {
        match signal {
            Signal::Update if !matches!(self.phase, Phase::Steering) => {
                self.batch.get_or_insert(now + BATCH);
                Action::Wait
            }
            Signal::Update | Signal::BatchReady => {
                self.batch = None;
                Action::Publish
            }
            Signal::Steer => {
                self.phase = Phase::Steering;
                self.batch = None;
                Action::Publish
            }
            Signal::Cancel => {
                self.cancel(now);
                Action::Cancel
            }
            Signal::CancelExpired => Action::Timeout,
        }
    }

    pub fn after_publish(&mut self, has_open_tools: bool, now: Instant) -> Action {
        if matches!(self.phase, Phase::Steering) && !has_open_tools {
            self.cancel(now);
            Action::Cancel
        } else {
            Action::Wait
        }
    }

    fn cancel(&mut self, now: Instant) {
        self.phase = Phase::Cancelling {
            deadline: now + CANCEL_GRACE,
        };
    }
}
