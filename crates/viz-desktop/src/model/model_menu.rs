//! Pure state and list derivation for the unified model picker.
//!
//! The picker edits one *scope*: a fresh draft (any harness, options sent on
//! launch) or an open thread (its own harness only, options applied through
//! `thread.configure`). Its harness rail offers a Favorites tab and one tab per
//! configured harness, default first. A non-empty search spans every harness at
//! once and hides the rail; the list itself derives in [`super::model_list`].
//! Reasoning effort, extra ACP options, speed and
//! access mode derive from the *previewed* model in
//! [`super::model_options`].

use std::collections::BTreeSet;

use viz_client::protocol::{ConfigOption, Harness, HarnessStatus, OptionKind};

use super::{Model, ModelPicker, OptionRole, OptionTarget, option_role};

/// `open_picker` key of the model picker popover.
pub const MODEL_PICKER: &str = "model";
/// `open_picker` key of the standalone access-mode popover (Home strip).
pub const ACCESS_PICKER: &str = "access";

static READY: HarnessStatus = HarnessStatus::Ready;

/// A tab in the picker's harness rail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RailTab {
    /// Starred models across every harness.
    Favorites,
    /// One configured harness by id.
    Harness(String),
}

impl RailTab {
    /// Stable element key of the tab's rail button.
    #[must_use]
    pub fn key(&self) -> String {
        match self {
            Self::Favorites => "model-rail-favorites".to_owned(),
            Self::Harness(id) => format!("model-rail-{id}"),
        }
    }
}

/// Which picker column owns the keyboard. Focus itself stays in the search
/// field; the column decides where arrows and Enter go.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PickerColumn {
    /// Harness rail.
    Rail,
    /// Search field and model list.
    #[default]
    Models,
    /// Reasoning effort and extra options.
    Options,
}

/// A model of one harness.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ModelRef {
    /// Harness id.
    pub harness: String,
    /// Model option value.
    pub value: String,
}

impl ModelRef {
    /// Key stored in `favoriteModels`: `harness:value`.
    #[must_use]
    pub fn favorite_key(&self) -> String {
        format!("{}:{}", self.harness, self.value)
    }

    /// Parses a `favoriteModels` entry.
    #[must_use]
    pub fn from_favorite(key: &str) -> Option<Self> {
        let (harness, value) = key.split_once(':')?;
        Some(Self {
            harness: harness.to_owned(),
            value: value.to_owned(),
        })
    }

    /// Stable element key of the model's list row.
    #[must_use]
    pub fn row_key(&self) -> String {
        format!("model-{}-{}", self.harness, self.value)
    }
}

/// Pure state of the open model picker.
#[derive(Debug, Clone, Default)]
pub struct ModelMenuState {
    /// Search text.
    pub query: String,
    /// Rail tab; `None` shows the scope's own harness.
    pub tab: Option<RailTab>,
    /// Column owning the keyboard.
    pub column: PickerColumn,
    /// Highlighted (previewed) model; the selected model when unset.
    pub highlight: Option<ModelRef>,
    /// Highlighted row in the options column.
    pub option_cursor: usize,
    /// Select option whose side submenu is open.
    pub submenu: Option<String>,
    /// Highlighted choice in the side submenu.
    pub submenu_cursor: usize,
    /// Folded groups the user expanded, keyed `harness/group`.
    pub expanded: BTreeSet<String>,
    /// Model value whose thinking levels are being fetched.
    pub loading_levels: Option<String>,
    /// Whether the footer's access-mode popover is open.
    pub access_open: bool,
    /// Access mode hovered or focused in the access popover.
    pub access_preview: Option<usize>,
}

/// What the picker edits: a draft or an open thread, its harness and options.
#[derive(Debug, Clone, Copy)]
pub struct PickerScope<'a> {
    /// Draft or thread.
    pub target: OptionTarget<'a>,
    /// Harness id the scope runs on.
    pub harness: &'a str,
    /// The harness's catalog entry, when listed.
    pub entry: Option<&'a Harness>,
    /// Current options: the thread's live ones, or the harness catalog.
    pub options: &'a [ConfigOption],
    /// Threads keep their harness; drafts may switch.
    pub locked: bool,
}

impl<'a> PickerScope<'a> {
    /// The scope's model and thinking projection.
    #[must_use]
    pub fn picker(&self) -> Option<ModelPicker<'a>> {
        ModelPicker::new(self.target, self.options)
    }

    /// The scope's selected model.
    #[must_use]
    pub fn selected(&self) -> Option<ModelRef> {
        self.picker()?.model_value().map(|value| ModelRef {
            harness: self.harness.to_owned(),
            value: value.to_owned(),
        })
    }

    /// Display name of the scope's harness.
    #[must_use]
    pub fn harness_name(&self) -> &'a str {
        self.entry.map_or(self.harness, |entry| &entry.name)
    }
}

/// The picker's scope for the current selection, if it has one.
#[must_use]
pub fn picker_scope(model: &Model) -> Option<PickerScope<'_>> {
    if !model.connected() {
        return None;
    }
    if let Some(draft) = &model.selection.new_thread {
        let entry = draft
            .active_harness(model.harnesses())
            .filter(|_| !draft.sending)?;
        return Some(PickerScope {
            target: OptionTarget::Draft(draft),
            harness: &entry.id,
            entry: Some(entry),
            options: &entry.options,
            locked: false,
        });
    }
    let thread = model.selection.selected?;
    let state = model.thread_state()?;
    Some(PickerScope {
        target: OptionTarget::Thread(thread),
        harness: &state.harness,
        entry: model.harnesses().iter().find(|h| h.id == state.harness),
        options: &state.options,
        locked: true,
    })
}

/// Configured harnesses in rail order: default first, then configured order.
#[must_use]
pub fn rail_harnesses(harnesses: &[Harness]) -> Vec<&Harness> {
    let mut ordered: Vec<_> = harnesses.iter().collect();
    ordered.sort_by_key(|harness| !harness.default);
    ordered
}

/// Whether a harness in `status` can start work.
#[must_use]
pub const fn status_usable(status: &HarnessStatus) -> bool {
    matches!(
        status,
        HarnessStatus::Unchecked | HarnessStatus::Ready | HarnessStatus::LimitReached { .. }
    )
}

/// A short title and detail explaining a harness status that needs attention.
#[must_use]
pub fn status_note(status: &HarnessStatus) -> Option<(&'static str, String)> {
    match status {
        HarnessStatus::Unchecked | HarnessStatus::Ready => None,
        HarnessStatus::NotInstalled { message } => Some(("Not installed", message.clone())),
        HarnessStatus::SignedOut { message } => Some(("Signed out", message.clone())),
        HarnessStatus::LimitReached { message, reset } => Some((
            "Usage limit reached",
            reset.as_ref().map_or_else(
                || message.clone(),
                |reset| format!("{} ({reset})", message.trim_end_matches('.')),
            ),
        )),
        HarnessStatus::Failed { message } => Some(("Unavailable", message.clone())),
    }
}

/// One harness's models as the picker sees them.
#[derive(Debug, Clone, Copy)]
pub struct Catalog<'a> {
    /// Harness id.
    pub id: &'a str,
    /// Display name.
    pub name: &'a str,
    /// Last checked status.
    pub status: &'a HarnessStatus,
    /// Options: the scope's for its own harness, else the catalog's.
    pub options: &'a [ConfigOption],
    /// Whether this is the scope's harness.
    pub own: bool,
}

impl<'a> Catalog<'a> {
    /// The harness's model selector.
    #[must_use]
    pub fn model_option(&self) -> Option<&'a ConfigOption> {
        self.options.iter().find(|option| {
            option_role(option) == OptionRole::Model && option.kind == OptionKind::Select
        })
    }
}

/// Every harness's catalog in rail order, with the scope's own options for its
/// harness (including one missing from the list).
#[must_use]
pub fn catalogs<'a>(model: &'a Model, scope: &PickerScope<'a>) -> Vec<Catalog<'a>> {
    let mut list: Vec<_> = rail_harnesses(model.harnesses())
        .into_iter()
        .map(|harness| {
            let own = harness.id == scope.harness;
            Catalog {
                id: &harness.id,
                name: &harness.name,
                status: &harness.status,
                options: if own { scope.options } else { &harness.options },
                own,
            }
        })
        .collect();
    if !list.iter().any(|catalog| catalog.own) {
        list.insert(
            0,
            Catalog {
                id: scope.harness,
                name: scope.harness_name(),
                status: &READY,
                options: scope.options,
                own: true,
            },
        );
    }
    if scope.locked {
        list.sort_by_key(|catalog| !catalog.own);
    }
    list
}
