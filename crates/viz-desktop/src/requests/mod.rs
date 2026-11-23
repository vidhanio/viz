//! Inline approval controls and the composer's elicitation drawer.

mod approval;
mod question;

pub use approval::approval_rows;
pub use question::{drawer as question_drawer, on_draft_key, sync};
