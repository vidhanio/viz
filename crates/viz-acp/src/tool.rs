//! Converts runner content and terminal reports at the ACP boundary.

mod normalize;
mod patch;
mod review;

pub use normalize::{normalize_output, normalize_stored_tool};
pub use patch::{ToolFields, ToolPatch, ToolReport};
use viz_protocol::{ContentBlock, ContentKind, Exit, Terminal};

use crate::schema;

pub fn block(content: schema::ContentBlock) -> ContentBlock {
    let kind = match &content {
        schema::ContentBlock::Text(_) => ContentKind::Text,
        schema::ContentBlock::Image(_) => ContentKind::Image,
        schema::ContentBlock::Audio(_) => ContentKind::Audio,
        schema::ContentBlock::ResourceLink(_) => ContentKind::ResourceLink,
        schema::ContentBlock::Resource(_) => ContentKind::Resource,
        _ => ContentKind::Other,
    };
    let payload = serde_json::to_value(content).expect("ACP content blocks contain JSON values");
    ContentBlock { kind, payload }
}

pub fn host_output(terminal: &mut Terminal, response: schema::TerminalOutputResponse) {
    terminal.output = response.output;
    terminal.truncated = response.truncated;
    terminal.exit = response.exit_status.map(|status| Exit {
        code: status.exit_code.map(i64::from),
        signal: status.signal,
    });
}
