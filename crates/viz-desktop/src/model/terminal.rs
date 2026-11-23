//! Dock layout, scoped tab membership, and desired terminal attachments.

use std::collections::BTreeSet;

use viz_client::protocol::{InteractiveScope, TerminalId, TerminalScope, TerminalSummary};

use super::Model;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Split {
    Right,
    Down,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    Tab,
    Split(TerminalId, Split),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Accessory {
    #[default]
    None,
    Find,
    Commands,
}

#[derive(Debug, Clone, Default)]
pub struct Dock {
    pub environment: String,
    pub listed: bool,
    pub sessions: Vec<TerminalSummary>,
    pub active: Option<TerminalId>,
    pub attached: BTreeSet<TerminalId>,
    pub groups: Vec<(Split, Vec<TerminalId>)>,
    pub auto_create: bool,
    pub creating: Option<Place>,
    pub panel_was_open: bool,
    pub accessory: Accessory,
}

impl Dock {
    pub fn panes(&self, id: TerminalId) -> (Split, &[TerminalId]) {
        self.groups
            .iter()
            .find(|(_, panes)| panes.contains(&id))
            .map_or((Split::Right, &[]), |(split, panes)| (*split, panes))
    }

    pub fn place(&mut self, id: TerminalId, place: Place) {
        if let Place::Split(anchor, direction) = place {
            if let Some((split, panes)) = self.groups.iter_mut().find(|(_, p)| p.contains(&anchor))
            {
                *split = direction;
                panes.push(id);
            } else {
                self.groups.push((direction, vec![anchor, id]));
            }
        }
        self.active = Some(id);
    }

    pub fn summary(&self, id: TerminalId) -> Option<&TerminalSummary> {
        self.sessions.iter().find(|terminal| terminal.id == id)
    }

    pub fn visible(model: &Model, terminal: &TerminalSummary) -> bool {
        match &terminal.scope {
            TerminalScope::Thread { thread } | TerminalScope::Agent { thread } => {
                model.selection.selected == Some(*thread)
            }
            TerminalScope::Project { project } => model.project_path() == Some(project),
        }
    }
}

pub fn scope(model: &Model, project: bool) -> Option<InteractiveScope> {
    if project {
        return model
            .project_path()
            .map(|project| InteractiveScope::Project {
                project: project.to_owned(),
            });
    }
    model
        .selection
        .selected
        .map(|thread| InteractiveScope::Thread { thread })
        .or_else(|| scope(model, true))
}

/// Keep space for the transcript and composer in short windows.
pub fn max_height(model: &Model) -> f32 {
    (model.panels.window_height - 180.0).clamp(120.0, 600.0)
}

pub fn height(model: &Model) -> f32 {
    model.panels.terminal_height.min(max_height(model))
}
