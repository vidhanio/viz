//! Identifiers. Numeric IDs are allocated from one upwards, so zero is
//! rejected wherever one is decoded.

use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, de::Error as _};

macro_rules! ids {
    ($($(#[$meta:meta])* $name:ident;)*) => {$(
        $(#[$meta])*
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
        )]
        #[serde(transparent)]
        pub struct $name(
            #[serde(deserialize_with = "positive")]
            #[schemars(range(min = 1))]
            pub u64,
        );

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }
    )*};
}

ids! {
    /// A thread id.
    ThreadId;
    /// A run id.
    RunId;
    /// A transcript entry id.
    EntryId;
    /// A harness request's id.
    RequestId;
}

fn positive<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u64, D::Error> {
    match u64::deserialize(deserializer)? {
        0 => Err(D::Error::custom("an id starts at 1")),
        id => Ok(id),
    }
}

/// A delegated task's id, allocated by the environment that owns the parent
/// thread.
#[derive(
    Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(transparent)]
pub struct TaskId(pub String);

impl TaskId {
    /// The id as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for TaskId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}
