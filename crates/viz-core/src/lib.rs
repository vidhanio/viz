//! Pure decisions for the viz workspace. The shell supplies persisted state and
//! interprets the returned changes and effects, with no I/O in these modules.

pub mod fork;
pub mod handoff;
pub mod lifecycle;
pub mod options;
pub mod placement;
pub mod projects;
pub mod runs;
pub mod tasks;
pub mod threads;
pub mod titles;
