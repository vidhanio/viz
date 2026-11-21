use toasty::Model;

/// The owner persists the request until it has a receipt, and the outcome until
/// it has delivered it.
#[derive(Debug, Model)]
#[table = "agent_tasks"]
pub(crate) struct TaskRow {
    #[key]
    pub(super) id: String,
    pub(super) parent: u64,
    pub(super) parent_run: Option<u64>,
    pub(super) environment: String,
    pub(super) thread: Option<u64>,
    pub(super) run: Option<u64>,
    #[default("queued".to_owned())]
    pub(super) status: String,
    pub(super) title: String,
    pub(super) result: Option<String>,
    pub(super) error: Option<String>,
    pub(super) payload: Option<String>,
    pub(super) request_id: Option<String>,
    #[default("pending".to_owned())]
    pub(super) phase: String,
    #[default("prompt".to_owned())]
    pub(super) delivery: String,
    pub(super) wake_run: Option<u64>,
}

/// A peer may disconnect after asking to cancel. The receiving environment
/// still drains the subtree.
#[derive(Debug, Model)]
#[table = "stopped_task_runs"]
#[key(thread, run)]
pub(crate) struct StoppedTaskRun {
    pub(super) thread: u64,
    pub(super) run: u64,
    #[default(false)]
    pub(super) handled: bool,
}
