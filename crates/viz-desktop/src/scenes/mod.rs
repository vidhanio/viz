//! Deterministic `AppSpec` scenes building pure [`crate::model::Model`]
//! fixtures for headless visual verification across dark and light themes.

mod data;
mod fixtures;
mod git;
mod home_composer;
mod model_picker;
mod overlays;
mod projects;
mod requests;
mod sidebar;
mod timeline;
mod timeline_cards;

use crate::{app::Desktop, motion_scene::MotionDemo};

type Scene = (&'static str, fn(&mut Desktop));

/// All registered headless preview scenes.
pub const SCENES: &[Scene] = &[
    ("workspace", workspace),
    ("git-details", git::full),
    ("git-details-compact", git::compact),
    ("git-details-essential", git::essential),
    ("git-branches", git::branch),
    ("git-actions", git::actions),
    ("git-commit", git::commit),
    ("git-checks", git::checks),
    ("git-diff", git::unified),
    ("git-diff-split", git::split),
    ("components", components),
    ("components-reduced", components_reduced),
    ("request-approval", requests::approval),
    ("request-question", requests::question),
    ("request-multi-step", requests::multi_step),
    ("motion", motion),
    ("motion-reduced", motion_reduced),
    ("activity", activity),
    ("worklog", worklog),
    ("timeline-populated", timeline::populated),
    ("timeline-long", timeline::long),
    ("timeline-expanded", timeline::expanded),
    ("timeline-working", timeline::working),
    ("timeline-status", timeline::status_line),
    ("file-preview", timeline::file_preview),
    ("entry-output", timeline::entry_output),
    ("user-markdown", timeline::user_markdown),
    ("timeline-cards", timeline_cards::cards),
    ("selectors", model_picker::claude),
    ("model-search", model_picker::search),
    ("model-picker-codex", model_picker::codex),
    ("model-picker-multi", model_picker::multi),
    ("model-picker-favorites", model_picker::favorites),
    (
        "model-picker-favorites-empty",
        model_picker::favorites_empty,
    ),
    ("model-picker-search-empty", model_picker::search_empty),
    ("model-picker-submenu", model_picker::submenu),
    ("model-picker-access", model_picker::access),
    ("model-picker-status", model_picker::status),
    ("model-picker-locked", model_picker::locked),
    ("new-agent", overlays::new_agent),
    ("context-menu", overlays::context_menu),
    ("context-menu-edge", overlays::context_menu_edge),
    ("palette", overlays::palette),
    ("palette-open", overlays::palette_open),
    ("project-sources", projects::sources),
    ("project-folders", projects::folders),
    ("project-new-folder", projects::new_folder),
    ("project-repositories", projects::repositories),
    ("project-cloning", projects::cloning),
    ("project-settings", projects::settings),
    ("project-icon", projects::icon),
    ("project-icon-colors", projects::colors),
    ("project-sessions", projects::sessions),
    ("attachments", overlays::attachments),
    ("slash", overlays::slash),
    ("mentions", overlays::mentions),
    ("empty", overlays::empty),
    ("loading", overlays::loading),
    ("onboarding", overlays::onboarding),
    ("subagents", subagents),
    ("thread-details-subagents", thread_details_subagents),
    ("agents", agents),
    ("subagent", subagent),
    ("delete", overlays::delete),
    ("overflow", overlays::overflow),
    ("projects", overlays::projects),
    ("draft-project", overlays::draft_project),
    ("draft-worktree", overlays::draft_worktree),
    ("move-thread", overlays::move_thread),
    ("environments", overlays::environments),
    ("offline", overlays::offline),
    ("sidebar-triage", sidebar::triage),
    ("sidebar-filter", sidebar::filter),
    ("sidebar-hover", sidebar::hover),
    ("sidebar-context", sidebar::context),
    ("sidebar-undo", sidebar::undo),
    ("sidebar-collapsed", sidebar::collapsed),
    ("home-no-project", home_composer::home_no_project),
    ("home-project-menu", home_composer::home_project_menu),
    ("home-project", home_composer::home_project),
    ("home-empty", home_composer::home_empty),
    ("composer-chips", home_composer::composer_chips),
    ("composer-markdown", home_composer::composer_markdown),
    ("composer-at", home_composer::composer_at),
    ("composer-slash", home_composer::composer_slash),
    ("composer-dollar", home_composer::composer_dollar),
    ("composer-hash", home_composer::composer_hash),
    ("composer-queued", home_composer::composer_queued),
];

fn components(s: &mut Desktop) {
    s.motion_demo = Some(MotionDemo::components());
}

fn components_reduced(s: &mut Desktop) {
    s.reduced_motion = Some(true);
    s.motion_demo = Some(MotionDemo::components());
}

fn motion(s: &mut Desktop) {
    s.motion_demo = Some(MotionDemo::default());
}

fn motion_reduced(s: &mut Desktop) {
    s.reduced_motion = Some(true);
    s.motion_demo = Some(MotionDemo::default());
}

fn workspace(s: &mut Desktop) {
    fixtures::seed_workspace(&mut s.model);
    s.sync_widgets_from_model();
}

fn activity(s: &mut Desktop) {
    fixtures::seed_activity(&mut s.model);
    s.sync_widgets_from_model();
}

fn worklog(s: &mut Desktop) {
    fixtures::seed_worklog(&mut s.model);
    s.sync_widgets_from_model();
}

fn subagents(s: &mut Desktop) {
    fixtures::seed_subagents(&mut s.model);
    s.sync_widgets_from_model();
}

fn thread_details_subagents(s: &mut Desktop) {
    fixtures::seed_thread_details_subagents(&mut s.model);
    s.sync_widgets_from_model();
}

fn agents(s: &mut Desktop) {
    fixtures::seed_agents(&mut s.model);
    s.sync_widgets_from_model();
}

fn subagent(s: &mut Desktop) {
    fixtures::seed_subagent(&mut s.model);
    s.sync_widgets_from_model();
}
