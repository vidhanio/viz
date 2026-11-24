//! ACP tools use the same authenticated dispatch as clients; task ownership
//! comes from the MCP grant.

mod inputs;
mod views;

use std::{convert::identity, sync::Arc, time::Duration};

use futures_util::future::BoxFuture;
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use viz_protocol::{
    AnswerRequest, AnsweredBy, Cleared, CreateProject, DelegateInput, Empty, EntryAnchor, EntryId,
    EnvironmentList, ForkThread, Forked, HarnessList, HarnessOptions, LaunchThread, Launched,
    ListBranches, ListEntries, ListHarnesses, ListProjects, ListThreads, ListWorktrees, Method,
    MoveThread, ReadThreadState, RemoveProject, RemoveWorktree, RequestAnswer, RequestId,
    RetitleThread, SearchThreads, SendInput, Shelf, StopThread, TaskId, TaskResult, ThreadId,
    ThreadStateResult, TriageThread, UpdateProject, UpdateThread, WaitThread,
};

use self::inputs::{SendText, ThreadUpdate, ToolInput, WaitRun};
use crate::{
    acp::mcp::{Dispatch, ToolDefinition, ToolResult, ToolScope},
    environments::Peers,
    error::Error,
    rpc::{CallCtx, Handler},
    tasks::Tasks,
    wire::decode,
    workspace::Workspace,
};

const LISTED_MODELS: usize = 24;
const LISTED_THREADS: u16 = 50;
const READ_ENTRIES: u16 = 20;
/// MCP clients give up on a call after about a minute, and viz answers without
/// a stream, so it cannot report progress meanwhile.
const MAX_WAIT: u64 = 45_000;
const DEFAULT_WAIT: u64 = 30_000;

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
    ($($name:literal, $method:ty, $input:ty, $link:expr, $view:expr, $description:literal;)+) => {
        fn rpc_definitions() -> Vec<ToolDefinition> {
            vec![$(definition::<Target<$input>>($name, $description)),+]
        }

        impl Tools {
            async fn call_rpc(&self, name: &str, args: Value) -> Result<ToolResult, Error> {
                match name {
                    $($name => self.rpc::<$method, $input, _>(args, $link, $view).await,)+
                    _ => Err(Error::not_found(format!("unknown tool {name}"))),
                }
            }
        }
    };
}

const fn no_link<T>(_: &T) -> Option<ThreadId> {
    None
}

rpc_tools! {
    "thread_state", ReadThreadState, ReadThreadState, no_link, |result: ThreadStateResult| views::state(result.state),
        "Read what a thread is doing now: status, directory, selected options, queued inputs, and the end of its live output.";
    "thread_search", SearchThreads, SearchThreads, no_link, identity,
        "Search all thread transcripts in an environment. Read a hit with thread_read.";
    "thread_launch", LaunchThread, LaunchThread, |response: &Launched| Some(response.thread), identity,
        "Start an independent thread for the user. Use delegate_task instead when you need its result.";
    "thread_fork", ForkThread, ForkThread, |forked: &Forked| Some(forked.thread), identity,
        "Copy a thread's history into a new idle thread: up to its head, through an entry's run, or before a prompt ({\"at\":\"before\",\"entry\":…}). Pass worktree {\"mode\":\"new\"} to give it its own checkout.";
    "thread_send", SendInput, SendText, no_link, identity,
        "Send an input to an existing thread. mode \"steer\" ends its current turn for this input; otherwise it queues. Returns the run to wait for.";
    "thread_wait", WaitThread, WaitRun, no_link, views::waited,
        "Wait up to 45 seconds for a thread's run to end. Pass run to wait for one input, or omit it for the latest. When timedOut is set, call again.";
    "thread_stop", StopThread, StopThread, no_link, identity,
        "Stop one run of a thread, or its active run and everything queued if run is omitted.";
    "thread_update", UpdateThread, ThreadUpdate, no_link, identity,
        "Rename a thread.";
    "thread_retitle", RetitleThread, RetitleThread, no_link, identity,
        "Replace a thread's title by one from its harness or latest prompt.";
    "thread_triage", TriageThread, TriageThread, no_link, identity,
        "Change where a thread stands in the user's list: settle finished work, reopen it, snooze it until a time in Unix milliseconds, pin, arrange, archive, or mark it read. Returns the state before, which the restore action puts back.";
    "thread_move", MoveThread, MoveThread, no_link, identity,
        "Move an idle project thread to a new worktree ({\"mode\":\"new\"}), an existing one, or back to the project checkout ({\"mode\":\"local\"}). Its conversation continues there.";
    "project_list", ListProjects, ListProjects, no_link, identity,
        "List an environment's projects, with their paths and branches.";
    "project_create", CreateProject, CreateProject, no_link, identity,
        "Register an existing directory as a project.";
    "project_update", UpdateProject, UpdateProject, no_link, identity,
        "Rename a project, given by its absolute path.";
    "project_remove", RemoveProject, RemoveProject, no_link, identity,
        "Remove a project from the list. Its files and threads stay.";
    "project_branches", ListBranches, ListBranches, no_link, identity,
        "List a project's local and remote-tracking branches, usable as a worktree base.";
    "worktree_list", ListWorktrees, ListWorktrees, no_link, identity,
        "List the checkouts of a project's repository, with their branches and the threads running in each.";
    "worktree_remove", RemoveWorktree, RemoveWorktree, no_link, identity,
        "Remove a worktree viz created once no thread runs in it. Refuses uncommitted changes unless force is set; the branch is kept.";
}

pub(crate) fn definitions() -> Vec<ToolDefinition> {
    let mut definitions = rpc_definitions();
    definitions.extend([
        definition::<Empty>("environment_list", "List the local environment and the configured peers, with their reachability."),
        definition::<Target<SpawnOptions>>("harness_list", "List the harnesses threads can run. Pass a harness and search to find its models, or a harness and model for its thinking levels."),
        definition::<Target<FindThreads>>("thread_list", "List threads, the ones that matter most first. Filter by project or shelf."),
        definition::<Target<ReadEntries>>("thread_read", "Read a thread's transcript, newest page first. Long texts and tool outputs are shortened; pass entry to read one whole, or before for earlier ones."),
        definition::<DelegateInput>("delegate_task", "Delegate a self-contained task to a new child thread, which sees only what you write in text. Pass worktree {\"mode\":\"new\"} to isolate its changes, thread to give a child of yours another task, or environment to run it elsewhere. The result arrives as a new input here, so end your turn instead of polling unless you must wait."),
        definition::<Empty>("task_list", "List the tasks this thread delegated, without their results."),
        definition::<TaskStatus>("task_status", "Read one of this thread's tasks with its result. Set wait to wait up to 45 seconds for it to end; when timedOut is set, call again."),
        definition::<TaskKey>("task_cancel", "Cancel one of this thread's tasks and everything delegated from it. An unreachable task stays until it can be told."),
        definition::<TaskKey>("task_delete", "Remove one of this thread's finished tasks from its list. The child thread stays."),
        definition::<Empty>("task_clear", "Remove all of this thread's finished tasks from its list."),
        definition::<ChildAnswer>("thread_answer", "Answer an approval or question a thread you delegated is waiting on, listed under requests in its thread_state. Give a permission one of its option ids, or a question its fields or a decline."),
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
            "environment_list" => {
                let _: Empty = decode(args)?;
                output(
                    &EnvironmentList {
                        environments: self.peers.list().await,
                    },
                    None,
                )
            }
            "harness_list" => {
                let Target { environment, input } = decode(args)?;
                self.spawn_options(scope.thread, environment.as_deref(), input)
                    .await
            }
            "thread_list" => {
                let Target { environment, input } = decode(args)?;
                self.threads(environment.as_deref(), input).await
            }
            "thread_read" => {
                let Target { environment, input } = decode(args)?;
                self.read(environment.as_deref(), input).await
            }
            "task_list" => {
                let _: Empty = decode(args)?;
                let tasks = self.tasks.list(Some(scope.thread)).await?;
                output(&views::tasks(tasks), None)
            }
            "task_delete" => {
                let TaskKey { task } = decode(args)?;
                self.tasks.delete(&task, Some(scope.thread)).await?;
                output(&views::Deleted { deleted: task }, None)
            }
            "task_clear" => {
                let _: Empty = decode(args)?;
                let count = self.tasks.clear(scope.thread).await?;
                output(&Cleared { count }, None)
            }
            "delegate_task" => self.delegate_task(scope, decode(args)?).await,
            "task_status" => self.task_status(scope, decode(args)?).await,
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
            "thread_answer" => {
                let ChildAnswer {
                    thread,
                    request,
                    answer,
                } = decode(args)?;
                let input = AnswerRequest {
                    thread,
                    request,
                    answer,
                };
                let request = self
                    .workspace
                    .answer(input, AnsweredBy::Agent(scope.thread))
                    .await?;
                output(&request, Some((thread, self.peers.local.as_str())))
            }
            _ => self.call_rpc(name, args).await,
        }
    }

    async fn delegate_task(
        &self,
        scope: ToolScope,
        input: DelegateInput,
    ) -> Result<ToolResult, Error> {
        let key = CallCtx::internal().principal.key();
        let task = self
            .tasks
            .delegate(&key, scope.thread, Some(scope.run), input)
            .await?;
        let result = TaskResult { task };
        let link = result
            .task
            .thread
            .map(|thread| (thread, result.task.environment.as_str()));
        output(&result, link)
    }

    async fn task_status(&self, scope: ToolScope, input: TaskStatus) -> Result<ToolResult, Error> {
        let timeout = input.timeout_ms.unwrap_or(DEFAULT_WAIT).min(MAX_WAIT);
        let result = self
            .tasks
            .status(&input.task, Some(scope.thread), input.wait, Some(timeout))
            .await?;
        let environment = result.task.environment.clone();
        let link = result
            .task
            .thread
            .map(|thread| (thread, environment.as_str()));
        output(&views::task(result.task, result.timed_out), link)
    }

    fn dispatch<'a, M, T>(
        &'a self,
        environment: &'a str,
        input: T,
        wait: Duration,
    ) -> BoxFuture<'a, Result<M::Response, Error>>
    where
        M: Method + 'static,
        T: Serialize + Send + Sync + Into<M> + 'static,
        Arc<Workspace>: Handler<M>,
    {
        Box::pin(async move {
            if environment == self.peers.local {
                Handler::<M>::handle(&self.workspace, &CallCtx::internal(), input.into()).await
            } else {
                self.peers.call::<M>(environment, &input, wait).await
            }
        })
    }

    async fn rpc<M, T, V>(
        &self,
        args: Value,
        child: fn(&M::Response) -> Option<ThreadId>,
        view: fn(M::Response) -> V,
    ) -> Result<ToolResult, Error>
    where
        M: Method + 'static,
        T: ToolInput + Into<M>,
        V: Serialize,
        Arc<Workspace>: Handler<M>,
    {
        let Target::<T> { environment, input } = decode(args)?;
        let environment = environment.as_deref().unwrap_or(&self.peers.local);
        let (thread, wait) = (input.thread(), input.extra_wait());
        let result = self.dispatch::<M, T>(environment, input, wait).await?;
        let link = thread
            .or_else(|| child(&result))
            .map(|thread| (thread, environment));
        output(&view(result), link)
    }

    async fn threads(
        &self,
        environment: Option<&str>,
        input: FindThreads,
    ) -> Result<ToolResult, Error> {
        let request = ListThreads {
            archived: None,
            project: input.project,
        };
        let environment = environment.unwrap_or(&self.peers.local);
        let mut found = self
            .dispatch::<ListThreads, _>(environment, request, Duration::ZERO)
            .await?
            .threads;
        found.retain(|thread| {
            input
                .shelf
                .map_or(thread.shelf != Shelf::Archived, |shelf| {
                    thread.shelf == shelf
                })
        });
        let limit = input.limit.unwrap_or(LISTED_THREADS).clamp(1, 200);
        output(&views::threads(found, limit.into()), None)
    }

    async fn read(
        &self,
        environment: Option<&str>,
        input: ReadEntries,
    ) -> Result<ToolResult, Error> {
        let environment = environment.unwrap_or(&self.peers.local);
        let link = Some((input.thread, environment));
        if let Some(wanted) = input.entry {
            if environment == self.peers.local {
                let entry = self.workspace.read_entry_raw(input.thread, wanted).await?;
                return output(&views::whole(entry), link);
            }
            let request = ListEntries {
                thread: input.thread,
                anchor: EntryAnchor::Before {
                    entry: EntryId(wanted.0.saturating_add(1)),
                },
                limit: Some(1),
            };
            let mut page = self
                .dispatch::<ListEntries, _>(environment, request, Duration::ZERO)
                .await?;
            return match page.entries.pop() {
                Some(entry) if entry.id == wanted => output(&views::whole(entry), link),
                _ => Err(Error::not_found(format!(
                    "no entry {wanted} in thread {}",
                    input.thread
                ))),
            };
        }
        let request = ListEntries {
            thread: input.thread,
            anchor: input
                .before
                .map_or(EntryAnchor::Latest, |entry| EntryAnchor::Before { entry }),
            limit: Some(input.limit.unwrap_or(READ_ENTRIES).clamp(1, 100)),
        };
        let page = self
            .dispatch::<ListEntries, _>(environment, request, Duration::ZERO)
            .await?;
        output(&views::page(page), link)
    }

    async fn spawn_options(
        &self,
        thread: ThreadId,
        environment: Option<&str>,
        input: SpawnOptions,
    ) -> Result<ToolResult, Error> {
        let environment = environment.unwrap_or(&self.peers.local);
        let HarnessList { harnesses } = self
            .dispatch::<ListHarnesses, _>(environment, ListHarnesses {}, Duration::ZERO)
            .await?;
        let parent_harness = if input.harness.is_none() && environment == self.peers.local {
            let ThreadStateResult { state } = Handler::<ReadThreadState>::handle(
                &self.workspace,
                &CallCtx::internal(),
                ReadThreadState { thread },
            )
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
struct FindThreads {
    project: Option<String>,
    /// Only this shelf. By default every thread that is not archived.
    shelf: Option<Shelf>,
    /// At most this many, 50 by default.
    #[schemars(range(min = 1, max = 200))]
    limit: Option<u16>,
}

#[derive(Deserialize, JsonSchema)]
struct ReadEntries {
    thread: ThreadId,
    /// Read entries before this one.
    before: Option<EntryId>,
    /// At most this many, 20 by default.
    #[schemars(range(min = 1, max = 100))]
    limit: Option<u16>,
    /// Read only this entry, whole.
    entry: Option<EntryId>,
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

/// A reply to one of a child thread's pending requests.
#[derive(Deserialize, JsonSchema)]
struct ChildAnswer {
    thread: ThreadId,
    request: RequestId,
    answer: RequestAnswer,
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
    #[schemars(range(max = 45_000))]
    timeout_ms: Option<u64>,
}
