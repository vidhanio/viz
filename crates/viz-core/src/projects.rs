//! Pure project editing and launch preferences.

use viz_protocol::{
    DefaultCheckout, LaunchThread, ProjectAutoSettle, ProjectIcon, ProjectIconChange,
    ProjectSettings, ProjectSettingsPatch, Setting, WorktreeTarget,
};

#[derive(Debug, thiserror::Error)]
pub enum Rejection {
    #[error("project name must not be empty")]
    EmptyName,
    #[error("invalid project icon")]
    Icon,
    #[error("auto-settle days must be positive")]
    SettleDays,
    #[error("invalid branch prefix")]
    BranchPrefix,
}

/// Persisted project preferences. Uploaded icons are stored as
/// content-addressed blobs in the workspace blob store and referenced by
/// SHA-256 hex digest.
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Preferences {
    #[serde(flatten)]
    pub settings: ProjectSettings,
}

/// Apply a patch to persisted preferences using validated filesystem and
/// harness facts.
///
/// # Errors
/// Returns the invalid setting.
pub fn edit(
    mut current: Preferences,
    patch: ProjectSettingsPatch,
) -> Result<Preferences, Rejection> {
    if matches!(
        patch.auto_settle,
        Some(Setting::Set(ProjectAutoSettle::AfterDays { days: 0 }))
    ) {
        return Err(Rejection::SettleDays);
    }
    if let Some(Setting::Set(prefix)) = &patch.branch_prefix
        && (prefix.is_empty()
            || !prefix
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "/_-".contains(c)))
    {
        return Err(Rejection::BranchPrefix);
    }
    if let Some(icon) = patch.icon {
        current.settings.icon = match icon {
            ProjectIconChange::Automatic => ProjectIcon::Automatic,
            ProjectIconChange::OwnerAvatar => ProjectIcon::OwnerAvatar,
            ProjectIconChange::Lucide { name, color }
                if !name.is_empty() && name.len() <= 80 && color.len() <= 32 =>
            {
                ProjectIcon::Lucide { name, color }
            }
            ProjectIconChange::Emoji { emoji } if !emoji.trim().is_empty() && emoji.len() <= 64 => {
                ProjectIcon::Emoji { emoji }
            }
            ProjectIconChange::Uploaded { media_type, blob }
                if blob.len() == 64
                    && blob
                        .bytes()
                        .all(|b| b.is_ascii_digit() || matches!(b, b'a'..=b'f')) =>
            {
                ProjectIcon::Uploaded { media_type, blob }
            }
            ProjectIconChange::Lucide { .. }
            | ProjectIconChange::Emoji { .. }
            | ProjectIconChange::Uploaded { .. } => {
                return Err(Rejection::Icon);
            }
        };
    }
    let settings = &mut current.settings;
    settings.pinned = patch.pinned.unwrap_or(settings.pinned);
    settings.archived = patch.archived.unwrap_or(settings.archived);
    settings.default_checkout = patch.default_checkout.unwrap_or(settings.default_checkout);
    if let Some(options) = patch.default_options {
        settings.default_options = options;
    }
    if let Some(dirs) = patch.additional_directories {
        settings.additional_directories = dirs;
    }
    apply(&mut settings.order, patch.order);
    apply(&mut settings.default_harness, patch.default_harness);
    apply(&mut settings.default_model, patch.default_model);
    apply(&mut settings.base_branch, patch.base_branch);
    apply(&mut settings.branch_prefix, patch.branch_prefix);
    apply(&mut settings.auto_settle, patch.auto_settle);
    Ok(current)
}

fn apply<T>(value: &mut Option<T>, patch: Option<Setting<T>>) {
    if let Some(patch) = patch {
        *value = match patch {
            Setting::Set(value) => Some(value),
            Setting::Clear => None,
        };
    }
}

/// Apply project defaults to fields the caller omitted.
pub fn launch_defaults(args: &mut LaunchThread, settings: ProjectSettings) {
    if args.harness.is_none() {
        args.harness = settings.default_harness;
    }
    if args.model.is_none() {
        args.model = settings.default_model;
    }
    for (key, value) in settings.default_options {
        args.options.entry(key).or_insert(value);
    }
    if args.worktree.is_none() && settings.default_checkout == DefaultCheckout::NewWorktree {
        args.worktree = Some(WorktreeTarget::New {
            base: None,
            branch: None,
        });
    }
    if let Some(WorktreeTarget::New { base, .. }) = &mut args.worktree
        && base.is_none()
    {
        *base = settings.base_branch;
    }
}
