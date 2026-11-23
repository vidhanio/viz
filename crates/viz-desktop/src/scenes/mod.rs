//! Deterministic `AppSpec` scenes building pure [`crate::model::Model`]
//! fixtures for headless visual verification across dark and light themes.

mod fixtures;
mod overlays;

use crate::{app::Desktop, motion_scene::MotionDemo};

type Scene = (&'static str, fn(&mut Desktop));

/// All registered headless preview scenes.
pub const SCENES: &[Scene] = &[
    ("workspace", workspace),
    ("components", components),
    ("components-reduced", components_reduced),
    ("terminal-shell", crate::terminal::scene::shell),
    ("terminal-alternate", crate::terminal::scene::alternate),
    ("terminal-selection", crate::terminal::scene::selection),
    ("terminal-exited", crate::terminal::scene::exited),
    ("motion", motion),
    ("motion-reduced", motion_reduced),
    ("activity", activity),
    ("worklog", worklog),
    ("selectors", overlays::selectors),
    ("model-search", overlays::model_search),
    ("new-agent", overlays::new_agent),
    ("context-menu", overlays::context_menu),
    ("palette", overlays::palette),
    ("palette-open", overlays::palette_open),
    ("attachments", overlays::attachments),
    ("slash", overlays::slash),
    ("mentions", overlays::mentions),
    ("empty", overlays::empty),
    ("loading", overlays::loading),
    ("onboarding", overlays::onboarding),
    ("subagents", subagents),
    ("tasks-expanded", tasks_expanded),
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

fn tasks_expanded(s: &mut Desktop) {
    fixtures::seed_tasks_expanded(&mut s.model);
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
