//! Session settings, normalized from ACP config options and legacy modes.

use serde::{Deserialize, Serialize};

use crate::schema::Meta;

/// A slash command offered by the ACP session.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Command {
    /// Command name without the slash.
    pub name: String,
    /// What the command does.
    pub description: String,
    /// Optional argument hint.
    pub hint: Option<String>,
    /// Skill metadata when a v1 runner exposes the skill as a command.
    pub skill: Option<Skill>,
    /// Metadata supplied by the harness.
    pub meta: Option<Meta>,
}

/// A skill exposed by a v1 command extension.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Skill {
    pub path: Option<String>,
}
