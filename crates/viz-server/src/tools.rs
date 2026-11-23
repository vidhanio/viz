//! ACP tools use the same authenticated dispatch as clients; task ownership
//! comes from the MCP grant.

mod inputs;

use std::{sync::Arc, time::Duration};

use futures_util::future::BoxFuture;
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use viz_protocol::{
    CreateProject, Empty, EnvironmentList, HarnessList, HarnessOptions, LaunchThread, Launched,
    ListBranches, ListEntries, ListHarnesses, ListProjects, ListThreads, ListWorktrees, Method,
    MoveThread, ReadProject, ReadThreadState, RemoveProject, RemoveWorktree, SearchThreads,
    SendInput, StopThread, TaskId, TaskList, TaskResult, ThreadId, ThreadStateResult, TriageThread,
    UpdateProject, UpdateThread, WaitThread,
};

use self::inputs::{SendText, ThreadUpdate, ToolInput};
use crate::{
    acp::mcp::{Dispatch, ToolDefinition, ToolResult, ToolScope},
    environments::Peers,
    error::Error,
    rpc::Handler,
    tasks::{Delegate, Tasks},
    wire::decode,
    workspace::{
        Workspace,
        rpc::{
            ListThreads as ThreadList, ProjectCreate, ProjectKey, ProjectUpdate, Search,
            WorktreeRemove,
        },
    },
};

const LISTED_MODELS: usize = 24;

#[derive(Clone)]
pub(crate) struct Tools {
    workspace: Arc<Workspace>,
    peers: Arc<Peers>,
    tasks: Arc<Tasks>,
}

impl std::fmt::Debug for Tools {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("Tools").finish_non_exhaustive()
    }
}

macro_rules! rpc_tools {
    ($($name:literal, $method:ty, $input:ty, $link:expr, $description:literal;)+) => {
        fn rpc_definitions() -> Vec<ToolDefinition> {
            vec![$(definition::<Target<$input>>($name, $description)),+]
        }

        impl Tools {
            async fn call_rpc(&self, name: &str, args: Value) -> Result<ToolResult, Error> {
                match name {
                    $($name => self.rpc::<$method, $input>(args, $link).await,)+
                    _ => Err(Error::not_found(format!("unknown tool {name}"))),
                }
            }
        }
    };
}

rpc_tools! {
    "thread_list", ListThreads<'static>, ThreadList, |_| None, "List threads in the selected environment, optionally filtered by project.";
    "thread_read", ListEntries, ListEntries, |_| None, "Read recent thread entries. Pass before for earlier entries.";
    "thread_state", ReadThreadState, ReadThreadState, |_| None, "Read a thread's current state, ACP options, working directory, and queue.";
    "thread_search", SearchThreads<'static>, Search, |_| None, "Search thread transcripts in the selected environment.";
    "thread_launch", LaunchThread, LaunchThread, |response: &Launched| Some(response.thread), "Launch an independent ACP thread (use delegate_task for a result that returns here).";
    "thread_send", SendInput, SendText, |_| None, "Send a new input to an existing thread. Its previous task result is not changed.";
    "thread_wait", WaitThread, WaitThread, |_| None, "Wait for a thread run to settle; specify the run ID to wait for that exact input.";
    "thread_stop", StopThread, StopThread, |_| None, "Stop a specific thread run, or the active run and queue if run is omitted.";
    "thread_update", UpdateThread<'static>, ThreadUpdate, |_| None, "Rename a thread.";
    "thread_triage", TriageThread, TriageThread, |_| None, "Settle a finished thread, reopen it, snooze it until a time, pin, archive or mark it. Returns its state before, which the restore action puts back.";
    "project_list", ListProjects, Empty, |_| None, "List the configured projects in the selected environment.";
    "project_read", ReadProject<'static>, ProjectKey, |_| None, "Read a project by its absolute path in the selected environment.";
    "project_create", CreateProject<'static>, ProjectCreate, |_| None, "Register an existing directory as a project.";
    "project_update", UpdateProject<'static>, ProjectUpdate, |_| None, "Rename a project by its absolute path.";
    "project_remove", RemoveProject<'static>, ProjectKey, |_| None, "Remove a project record without deleting its files.";
    "project_branches", ListBranches<'static>, ProjectKey, |_| None, "List a Git project's local and remote-tracking branches, usable as a worktree base.";
    "worktree_list", ListWorktrees<'static>, ProjectKey, |_| None, "List the checkouts of a Git project's repository, with their branches and the threads running in each.";
    "worktree_remove", RemoveWorktree<'static>, WorktreeRemove, |_| None, "Remove a worktree viz created once no thread runs in it. Refuses uncommitted changes unless force is set; the branch is kept.";
    "thread_move", MoveThread, MoveThread, |_| None, "Move an idle project thread to a new worktree ({\"mode\":\"new\"}), an existing one, or back to the project checkout ({\"mode\":\"local\"}). Its conversation continues there.";
}

pub(crate) fn definitions() -> Vec<ToolDefinition> {
    let mut definitions = rpc_definitions();
    definitions.extend([
        definition::<Empty>("list_environments", "List the local and explicitly configured peer environments, with reachability."),
        definition::<Target<SpawnOptions>>("list_spawn_options", "List ACP harness IDs and display names. Pass a harness ID and model for model-specific thinking options, or search to filter models."),
        definition::<Delegate>("delegate_task", "Delegate a self-contained task from this thread. Pass a destination environment and project if needed, or reuse one of your existing child threads. The result arrives as a new input here."),
        definition::<Empty>("list_tasks", "List the durable tasks delegated by this thread."),
        definition::<TaskStatus>("task_status", "Read the status and result of one of this thread's tasks by task ID. Set wait to wait for completion."),
        definition::<TaskKey>("task_cancel", "Cancel one of this thread's tasks by task ID, including its nested work. An unreachable task remains pending until cancellation succeeds."),
    ]);
    definitions
}

fn definition<T: JsonSchema>(name: &'static str, description: &'static str) -> ToolDefinition {
    ToolDefinition {
        name,
        description,
        input_schema: json!(schema_for!(T)),
    }
}

impl Tools {
    pub(crate) const fn new(
        workspace: Arc<Workspace>,
        peers: Arc<Peers>,
        tasks: Arc<Tasks>,
    ) -> Self {
        Self {
            workspace,
            peers,
            tasks,
        }
    }

    async fn invoke(&self, scope: ToolScope, name: &str, args: Value) -> Result<ToolResult, Error> {
        match name {
            "list_environments" => {
                let _: Empty = decode(args)?;
                output(
                    &EnvironmentList {
                        environments: self.peers.list().await,
                    },
                    None,
                )
            }
            "list_spawn_options" => {
                let Target { environment, input } = decode(args)?;
                self.spawn_options(scope.thread, environment.as_deref(), input)
                    .await
            }
            "list_tasks" => {
                let _: Empty = decode(args)?;
                let tasks = self.tasks.list(Some(scope.thread)).await?;
                output(&TaskList { tasks }, None)
            }
            "delegate_task" => {
                let task = self
                    .tasks
                    .delegate(scope.thread, Some(scope.run), decode(args)?)
                    .await?;
                let result = TaskResult { task };
                let link = result
                    .task
                    .thread
                    .map(|thread| (thread, result.task.environment.as_str()));
                output(&result, link)
            }
            "task_status" => {
                let TaskStatus {
                    task,
                    wait,
                    timeout_ms,
                } = decode(args)?;
                let result = self
                    .tasks
                    .status(&task, Some(scope.thread), wait, timeout_ms)
                    .await?;
                let link = result
                    .task
                    .thread
                    .map(|thread| (thread, result.task.environment.as_str()));
                output(&result, link)
            }
            "task_cancel" => {
                let TaskKey { task } = decode(args)?;
                let task = self.tasks.cancel(&task, Some(scope.thread)).await?;
                let result = TaskResult { task };
                let link = result
                    .task
                    .thread
                    .map(|thread| (thread, result.task.environment.as_str()));
                output(&result, link)
            }
            _ => self.call_rpc(name, args).await,
        }
    }

    fn dispatch<'a, M, T>(
        &'a self,
        environment: &'a str,
        input: T,
        wait: Duration,
    ) -> BoxFuture<'a, Result<M::Response, Error>>
    where
        M: Method + 'static,
        T: Serialize + Send + Sync + Into<<Arc<Workspace> as Handler<M>>::Input> + 'static,
        Arc<Workspace>: Handler<M>,
    {
        Box::pin(async move {
            if environment == self.peers.local {
                Handler::<M>::handle(&self.workspace, input.into()).await
            } else {
                self.peers.call::<M>(environment, &input, wait).await
            }
        })
    }

    async fn rpc<M, T>(
        &self,
        args: Value,
        child: fn(&M::Response) -> Option<ThreadId>,
    ) -> Result<ToolResult, Error>
    where
        M: Method + 'static,
        T: ToolInput + Into<<Arc<Workspace> as Handler<M>>::Input>,
        Arc<Workspace>: Handler<M>,
    {
        let Target::<T> { environment, input } = decode(args)?;
        let environment = environment.as_deref().unwrap_or(&self.peers.local);
        let (thread, wait) = (input.thread(), input.extra_wait());
        let result = self.dispatch::<M, T>(environment, input, wait).await?;
        let link = thread
            .or_else(|| child(&result))
            .map(|thread| (thread, environment));
        output(&result, link)
    }

    async fn spawn_options(
        &self,
        thread: ThreadId,
        environment: Option<&str>,
        input: SpawnOptions,
    ) -> Result<ToolResult, Error> {
        let environment = environment.unwrap_or(&self.peers.local);
        let HarnessList { harnesses } = self
            .dispatch::<ListHarnesses, _>(environment, Empty {}, Duration::ZERO)
            .await?;
        let parent_harness = if input.harness.is_none() && environment == self.peers.local {
            let ThreadStateResult { state } =
                Handler::<ReadThreadState>::handle(&self.workspace, ReadThreadState { thread })
                    .await?;
            Some(state.harness)
        } else {
            None
        };
        let default_harness = parent_harness.as_deref().or_else(|| {
            harnesses
                .iter()
                .find(|harness| harness.default)
                .map(|harness| harness.id.as_str())
        });
        if input.harness.is_none() && input.model.is_none() && input.search.is_none() {
            return output(
                &SpawnCatalog {
                    default_harness,
                    harnesses: harnesses
                        .iter()
                        .map(|harness| HarnessLabel {
                            id: &harness.id,
                            name: &harness.name,
                        })
                        .collect(),
                },
                None,
            );
        }
        let name = input
            .harness
            .as_deref()
            .or(default_harness)
            .ok_or_else(|| Error::not_found("no default ACP harness; specify a harness"))?;
        let harness = harnesses
            .iter()
            .find(|harness| harness.id == name)
            .ok_or_else(|| Error::not_found(format!("no ACP harness {name}")))?;
        if let Some(model) = input.model {
            let selected = self
                .dispatch::<HarnessOptions, _>(
                    environment,
                    HarnessOptions {
                        harness: name.to_owned(),
                        model,
                    },
                    Duration::ZERO,
                )
                .await?;
            let model = selected
                .options
                .iter()
                .find(|option| option.category.as_deref() == Some("model"));
            let thinking = selected
                .options
                .iter()
                .find(|option| option.category.as_deref() == Some("thought_level"));
            return output(
                &SpawnSelection {
                    harness: name,
                    harness_name: &harness.name,
                    model: model.map(|option| &option.value),
                    thinking,
                },
                None,
            );
        }
        spawn_models(harness, input.search.as_deref())
    }
}

fn spawn_models(harness: &crate::wire::Harness, search: Option<&str>) -> Result<ToolResult, Error> {
    let model = harness
        .options
        .iter()
        .find(|option| option.category.as_deref() == Some("model"));
    let search = search.map(|query| query.trim().to_lowercase());
    let mut matching = model
        .into_iter()
        .flat_map(|option| &option.choices)
        .filter(|choice| {
            search.as_ref().is_none_or(|query| {
                [
                    Some(choice.value.as_str()),
                    Some(choice.name.as_str()),
                    choice.group.as_deref(),
                ]
                .into_iter()
                .flatten()
                .any(|field| field.to_lowercase().contains(query))
            })
        });
    let models = matching
        .by_ref()
        .take(LISTED_MODELS)
        .map(|choice| ListedModel {
            value: &choice.value,
            name: &choice.name,
            group: choice.group.as_deref(),
        })
        .collect();
    output(
        &SpawnModels {
            harness: &harness.id,
            harness_name: &harness.name,
            models,
            omitted_models: matching.count(),
        },
        None,
    )
}

impl Dispatch for Tools {
    fn call(
        &self,
        scope: ToolScope,
        name: String,
        args: Value,
    ) -> BoxFuture<'static, Result<ToolResult, String>> {
        let tools = self.clone();
        Box::pin(async move {
            tools
                .invoke(scope, &name, args)
                .await
                .map_err(|error| error.to_string())
        })
    }
}

fn output(value: &impl Serialize, link: Option<(ThreadId, &str)>) -> Result<ToolResult, Error> {
    Ok(ToolResult {
        text: serde_json::to_string(value)?,
        child: link.map(|(thread, _)| thread),
        child_environment: link.map(|(_, environment)| environment.to_owned()),
    })
}

#[derive(Deserialize, JsonSchema)]
struct Target<T> {
    /// Environment ID from `list_environments`. Omit for the environment
    /// running this thread.
    environment: Option<String>,
    #[serde(flatten)]
    input: T,
}

#[derive(Deserialize, JsonSchema)]
struct SpawnOptions {
    harness: Option<String>,
    model: Option<String>,
    search: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SpawnCatalog<'a> {
    default_harness: Option<&'a str>,
    harnesses: Vec<HarnessLabel<'a>>,
}

#[derive(Serialize)]
struct HarnessLabel<'a> {
    id: &'a str,
    name: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SpawnSelection<'a> {
    harness: &'a str,
    harness_name: &'a str,
    model: Option<&'a viz_protocol::OptionValue>,
    thinking: Option<&'a viz_protocol::ConfigOption>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SpawnModels<'a> {
    harness: &'a str,
    harness_name: &'a str,
    models: Vec<ListedModel<'a>>,
    omitted_models: usize,
}

#[derive(Serialize)]
struct ListedModel<'a> {
    value: &'a str,
    name: &'a str,
    group: Option<&'a str>,
}

#[derive(Deserialize, JsonSchema)]
struct TaskKey {
    task: TaskId,
}

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct TaskStatus {
    task: TaskId,
    #[serde(default)]
    wait: bool,
    #[schemars(range(max = 120_000))]
    timeout_ms: Option<u64>,
}
