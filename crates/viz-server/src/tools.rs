//! ACP tools use the same authenticated dispatch as clients; task ownership
//! comes from the MCP grant.

use std::sync::Arc;

use futures_util::future::BoxFuture;
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use viz_client::protocol::{
    EnvironmentList, ForkThread, HarnessList, HarnessOptions, HarnessOptionsResult, LaunchThread,
    ListEntries, ListGitBranches, ReadThreadState, RestoreRun, RewindThread, RunDiff, SendMode,
    StopThread, TaskList, TaskResult, ThreadDiff, ThreadStateResult, WaitThread,
};

use crate::{
    acp::mcp::Dispatch,
    environments::Peers,
    tasks::{Delegate, Tasks},
    wire::{RpcError, decode, encode},
    workspace::Workspace,
};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ToolDefinition {
    pub(crate) name: &'static str,
    pub(crate) description: &'static str,
    pub(crate) input_schema: Value,
}

const LISTED_MODELS: usize = 24;

#[derive(Debug, Clone, Copy)]
pub(crate) struct ToolScope {
    pub(crate) thread: u64,
    pub(crate) run: u64,
}

#[derive(Debug)]
pub(crate) struct ToolResult {
    pub(crate) text: String,
    pub(crate) child: Option<u64>,
    pub(crate) child_environment: Option<String>,
}

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
    ($($name:literal, $method:literal, $input:ty, $description:literal;)+) => {
        fn rpc_definitions() -> Vec<ToolDefinition> {
            vec![$(definition::<Target<$input>>($name, $description)),+]
        }

        impl Tools {
            async fn call_rpc(&self, name: &str, args: Value) -> Result<ToolResult, RpcError> {
                match name {
                    $($name => self.rpc::<$input>($method, args).await,)+
                    _ => Err(RpcError::not_found(format!("unknown tool {name}"))),
                }
            }
        }
    };
}

rpc_tools! {
    "thread_list", "threads.list", ThreadList, "List threads in the selected environment, optionally filtered by project.";
    "git_branches", "git.branches", ListGitBranches, "List local and remote-tracking branches of a registered project.";
    "run_diff", "run.diff", RunDiff, "Compare the Git working tree at the start and end of a run.";
    "run_restore", "run.restore", RestoreRun, "Restore the working tree of an idle thread to the selected run's start checkpoint.";
    "thread_rewind", "thread.rewind", RewindThread, "Restore the working tree and rewind the transcript of an idle thread.";
    "thread_fork", "thread.fork", ForkThread, "Branch off a settled run boundary with a fresh ACP session and optional new worktree.";
    "thread_diff", "thread.diff", ThreadDiff, "Compare the first and latest complete Git checkpoints of a thread.";
    "thread_read", "thread.entries", ListEntries, "Read recent thread entries. Pass before for earlier entries.";
    "thread_state", "thread.state", ReadThreadState, "Read a thread's current state, ACP options, working directory, and queue.";
    "thread_search", "threads.search", Search, "Search thread transcripts in the selected environment.";
    "thread_launch", "thread.launch", LaunchThread, "Launch an independent ACP thread (use delegate_task for a result that returns here).";
    "thread_send", "thread.send", Send, "Send a new input to an existing thread. Its previous task result is not changed.";
    "thread_wait", "thread.wait", WaitThread, "Wait for a thread run to settle; specify the run ID to wait for that exact input.";
    "thread_stop", "thread.stop", StopThread, "Stop a specific thread run, or the active run and queue if run is omitted.";
    "thread_update", "thread.update", ThreadUpdate, "Change a thread's title, archive or pin state.";
    "project_list", "projects.list", Empty, "List the configured projects in the selected environment.";
    "project_read", "project.read", Project, "Read a project by its absolute path in the selected environment.";
    "project_create", "project.create", ProjectCreate, "Register an existing directory as a project.";
    "project_update", "project.update", ProjectUpdate, "Rename a project by its absolute path.";
    "project_remove", "project.remove", Project, "Remove a project record without deleting its files.";
}

pub(crate) fn definitions() -> Vec<ToolDefinition> {
    let mut definitions = rpc_definitions();
    definitions.extend([
        definition::<Empty>("list_environments", "List the local and explicitly configured peer environments, with reachability."),
        definition::<Target<SpawnOptions>>("list_spawn_options", "List ACP harness IDs and display names. Pass a harness ID and model for model-specific thinking options, or search to filter models."),
        definition::<Delegate>("delegate_task", "Delegate a self-contained task from this thread. Pass a destination environment and project if needed, or reuse one of your existing child threads. The result arrives as a new input here."),
        definition::<Empty>("list_tasks", "List the durable tasks delegated by this thread."),
        definition::<TaskStatus>("task_status", "Read the status and result of one of this thread's tasks by task ID. Set wait to wait for completion."),
        definition::<TaskId>("task_cancel", "Cancel one of this thread's tasks by task ID, including its nested work. An unreachable task remains pending until cancellation succeeds."),
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

    async fn invoke(
        &self,
        scope: ToolScope,
        name: &str,
        args: Value,
    ) -> Result<ToolResult, RpcError> {
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
                    .map(|thread| (thread.0, result.task.environment.as_str()));
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
                    .map(|thread| (thread.0, result.task.environment.as_str()));
                output(&result, link)
            }
            "task_cancel" => {
                let TaskId { task } = decode(args)?;
                let task = self.tasks.cancel(&task, Some(scope.thread)).await?;
                let result = TaskResult { task };
                let link = result
                    .task
                    .thread
                    .map(|thread| (thread.0, result.task.environment.as_str()));
                output(&result, link)
            }
            _ => self.call_rpc(name, args).await,
        }
    }

    async fn dispatch(
        &self,
        environment: &str,
        method: &str,
        params: Value,
    ) -> Result<Value, RpcError> {
        if environment == self.peers.local {
            self.workspace.call(method, params).await
        } else {
            self.peers.call(environment, method, params).await
        }
    }

    async fn rpc<T: DeserializeOwned + Serialize>(
        &self,
        method: &str,
        args: Value,
    ) -> Result<ToolResult, RpcError> {
        let Target::<T> { environment, input } = decode(args)?;
        let environment = environment.as_deref().unwrap_or(&self.peers.local);
        let params = encode(input)?;
        let target = params.get("thread").and_then(Value::as_u64);
        let result = self.dispatch(environment, method, params).await?;
        let thread = target.or_else(|| result.get("thread").and_then(Value::as_u64));
        output(&result, thread.map(|thread| (thread, environment)))
    }

    async fn spawn_options(
        &self,
        thread: u64,
        environment: Option<&str>,
        input: SpawnOptions,
    ) -> Result<ToolResult, RpcError> {
        let environment = environment.unwrap_or(&self.peers.local);
        let HarnessList { harnesses } = decode(
            self.dispatch(environment, "harnesses.list", json!({}))
                .await?,
        )?;
        let parent_harness = if input.harness.is_none() && environment == self.peers.local {
            let state = self
                .workspace
                .call("thread.state", json!({"thread": thread}))
                .await?;
            Some(decode::<ThreadStateResult>(state)?.state.harness)
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
            .ok_or_else(|| RpcError::not_found("no default ACP harness; specify a harness"))?;
        let harness = harnesses
            .iter()
            .find(|harness| harness.id == name)
            .ok_or_else(|| RpcError::not_found(format!("no ACP harness {name}")))?;
        if let Some(model) = input.model {
            let options = self
                .dispatch(
                    environment,
                    "harnesses.options",
                    encode(HarnessOptions {
                        harness: name.to_owned(),
                        model,
                    })?,
                )
                .await?;
            let selected: HarnessOptionsResult = decode(options)?;
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

fn spawn_models(
    harness: &crate::wire::Harness,
    search: Option<&str>,
) -> Result<ToolResult, RpcError> {
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
                .map_err(|error| error.message)
        })
    }
}

fn output(value: &impl Serialize, link: Option<(u64, &str)>) -> Result<ToolResult, RpcError> {
    Ok(ToolResult {
        text: serde_json::to_string(value).map_err(RpcError::internal)?,
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

#[derive(Serialize, Deserialize, JsonSchema)]
struct Empty {}

#[derive(Serialize, Deserialize, JsonSchema)]
struct ThreadList {
    #[serde(skip_serializing_if = "Option::is_none")]
    archived: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    project: Option<String>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
struct Search {
    query: String,
    #[schemars(range(min = 1, max = 200))]
    #[serde(skip_serializing_if = "Option::is_none")]
    limit: Option<u16>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct Send {
    #[schemars(range(min = 1))]
    thread: u64,
    #[schemars(length(min = 1))]
    text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    mode: Option<SendMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    request_id: Option<String>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
struct ThreadUpdate {
    #[schemars(range(min = 1))]
    thread: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    archived: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pinned: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    unread: Option<bool>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
struct Project {
    project: String,
}

#[derive(Serialize, Deserialize, JsonSchema)]
struct ProjectCreate {
    path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
struct ProjectUpdate {
    project: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
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
    model: Option<&'a viz_client::protocol::OptionValue>,
    thinking: Option<&'a viz_client::protocol::ConfigOption>,
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
struct TaskId {
    task: String,
}

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct TaskStatus {
    task: String,
    #[serde(default)]
    wait: bool,
    #[schemars(range(max = 120_000))]
    timeout_ms: Option<u64>,
}
