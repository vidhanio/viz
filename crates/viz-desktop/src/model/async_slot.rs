//! Reusable asynchronous load/decode lifecycle state for pure model slots.

/// Explicit lifecycle state of an asynchronously fetched or decoded value.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum AsyncSlot<T> {
    /// No request has started yet.
    #[default]
    Idle,
    /// A request is currently in flight.
    Loading,
    /// The latest request succeeded with `T`.
    Ready(T),
    /// The latest request failed with a user-visible reason.
    Error(String),
}

impl<T> AsyncSlot<T> {
    /// Borrows the resolved value, if ready.
    #[must_use]
    pub const fn as_ready(&self) -> Option<&T> {
        match self {
            Self::Ready(value) => Some(value),
            Self::Idle | Self::Loading | Self::Error(_) => None,
        }
    }

    /// Transitions an idle or failed slot into [`Self::Loading`], returning
    /// `true` if a new request should be started.
    pub fn begin(&mut self) -> bool {
        if matches!(self, Self::Loading | Self::Ready(_)) {
            return false;
        }
        *self = Self::Loading;
        true
    }

    /// Marks the slot as resolved with `value`.
    pub fn set_ready(&mut self, value: T) {
        *self = Self::Ready(value);
    }

    /// Marks the slot as failed with `message`.
    pub fn set_error(&mut self, message: impl Into<String>) {
        *self = Self::Error(message.into());
    }
}
