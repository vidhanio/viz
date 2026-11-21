//! The server connection: a tokio runtime on its own thread that connects
//! (retrying while the server is down), watches the selected thread, and
//! pushes state into the UI.

mod requests;

use std::{cmp::Reverse, sync::Arc, time::Duration};

use rune::AppHandle;
use tokio::sync::mpsc;
use viz_client::{
    Client, Connection, Endpoint, Error,
    protocol::{
        CancelTask, ClientInfo, ClientKind, ConfigureThread, CreateProject, Entry, EntryId,
        ForkThread, ForkWorktree, HarnessOptions, LaunchThread, ListEnvironments, ListGitBranches,
        ListTasks, Notification, OptionValue, RemoveProject, RemoveQueuedInput, RestoreRun,
        RewindThread, RunBoundary, RunDiff, RunId, SearchFiles, SendInput, SteerQueuedInput,
        ThreadDiff, ThreadId, UpdateProject, UpdateQueuedInput, UpdateThread,
    },
};

use crate::{
    app::{Desktop, Status},
    attachments::{Source, Target as AttachmentTarget},
    diff_panel::{DiffFile, DiffScope},
};

/// How long to wait between connection attempts.
const RETRY: Duration = Duration::from_secs(3);
const NOT_FOUND: i64 = -32001;

/// What the UI asks of the server.
#[derive(Debug)]
pub enum Command {
    /// Watch this thread instead.
    Select(ThreadId),
    ClearSelection,
    Environment {
        environment: String,
        thread: Option<ThreadId>,
    },
    CancelTask(String),
    /// Bind and send the first input of the fresh composer.
    Create {
        draft: u64,
        params: LaunchThread,
    },
    /// A runner's options with `model` selected, for a draft that picked a
    /// model whose thinking levels the catalog doesn't know yet.
    HarnessOptions {
        harness: String,
        model: String,
    },
    /// A fresh draft's runner choices.
    LoadDraft(u64),
    Update(ThreadId, ThreadChange),
    Delete(ThreadId),
    Configure {
        thread: ThreadId,
        option: String,
        value: OptionValue,
    },
    Abort(ThreadId),
    Project(ProjectChange),
    ListBranches(String),
    Git(GitCommand),
    SearchFiles {
        thread: ThreadId,
        request: u64,
        query: String,
    },
    SearchThreads {
        request: u64,
        query: String,
    },
    LoadEarlier {
        thread: ThreadId,
        before: EntryId,
    },
    RemoveQueued {
        thread: ThreadId,
        run: RunId,
    },
    EditQueued {
        thread: ThreadId,
        run: RunId,
        text: String,
    },
    MoveQueued {
        thread: ThreadId,
        run: RunId,
        before: Option<RunId>,
    },
    SteerQueued(SteerQueuedInput),
    Attach {
        scope: u64,
        target: AttachmentTarget,
        source: Source,
    },
    /// Sends an input, including any locally prepared attachments.
    SendInput(SendInput),
    /// Show a desktop notification.
    Notify {
        title: String,
        body: &'static str,
    },
    /// While disconnected: try again now.
    Reconnect,
}

#[derive(Debug)]
pub enum ThreadChange {
    Title(String),
    Archive(bool),
    Pin(bool),
    Unread(bool),
    Project(Option<String>),
}

#[derive(Debug)]
pub enum ProjectChange {
    Add(String),
    Rename { id: String, name: String },
    Remove(String),
}

/// Diff inspection and checkpoint commands backed by the server's Git layer.
#[derive(Debug)]
pub enum GitCommand {
    ThreadDiff {
        thread: ThreadId,
    },
    RunDiff {
        thread: ThreadId,
        run: RunId,
        turn: usize,
        summary: bool,
    },
    RestoreRun {
        thread: ThreadId,
        run: RunId,
    },
    RewindThread {
        thread: ThreadId,
        run: RunId,
        prompt_text: String,
    },
    ForkThread {
        thread: ThreadId,
        run: RunId,
        boundary: RunBoundary,
        worktree: bool,
    },
}

/// Starts the connection thread. `first` hears once the first connection
/// attempt has reached the UI (so a headless snapshot can show its outcome).
pub fn spawn(
    ui: AppHandle<Desktop>,
    commands: mpsc::UnboundedReceiver<Command>,
    first: std::sync::mpsc::Sender<()>,
    endpoint: Result<Endpoint, String>,
) {
    std::thread::spawn(move || {
        match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(runtime) => runtime.block_on(run(&ui, commands, first, endpoint)),
            Err(e) => {
                tracing::error!(error = %e, "could not start the connection runtime");
                set_status(
                    &ui,
                    Status::Error(format!("Couldn’t start the connection thread: {e}")),
                );
                _ = first.send(());
            }
        }
    });
}

fn set_status(ui: &AppHandle<Desktop>, status: Status) {
    ui.update(move |s, tx| {
        s.status = status;
        s.update_title(tx);
    });
}

fn toast(ui: &AppHandle<Desktop>, title: &'static str, detail: String) {
    ui.update(move |s, tx| s.toast(tx, title, Some(detail)));
}

/// Tells `main` the first connection attempt's outcome is in the UI's queue.
fn signal(first: &mut Option<std::sync::mpsc::Sender<()>>) {
    if let Some(first) = first.take() {
        _ = first.send(());
    }
}

async fn run(
    ui: &AppHandle<Desktop>,
    mut commands: mpsc::UnboundedReceiver<Command>,
    first: std::sync::mpsc::Sender<()>,
    endpoint: Result<Endpoint, String>,
) {
    let mut first = Some(first);
    let endpoint = match endpoint {
        Ok(endpoint) => endpoint,
        Err(detail) => {
            set_status(ui, Status::Error(detail.clone()));
            signal(&mut first);
            while let Some(command) = commands.recv().await {
                if !offline(ui, command, &mut None, &mut None) {
                    set_status(ui, Status::Error(detail.clone()));
                }
            }
            return;
        }
    };
    let shown = endpoint.to_string();
    ui.update(move |s, _| s.endpoint = Some(shown));
    let mut selected = None;
    let mut environment = None;
    loop {
        let info = ClientInfo {
            kind: ClientKind::App,
            name: "desktop".into(),
        };
        let lost = match Client::connect(&endpoint, info, environment.clone()).await {
            Ok(connection) => {
                match session(ui, connection, &mut commands, &mut selected, &mut first).await {
                    Ended::Quit => return,
                    Ended::Environment { target, thread } => {
                        selected = thread;
                        environment = Some(target);
                        continue;
                    }
                    Ended::Lost(e) => {
                        toast(ui, "Lost the connection to the server", e.to_string());
                        e.to_string()
                    }
                }
            }
            Err(e) => e.to_string(),
        };
        tracing::debug!(%endpoint, "server connection unavailable; retrying");
        set_status(ui, Status::Error(lost));
        signal(&mut first);
        let retry = tokio::time::sleep(RETRY);
        tokio::pin!(retry);
        loop {
            tokio::select! {
                () = &mut retry => break,
                command = commands.recv() => match command {
                    Some(command) => if !offline(ui, command, &mut selected, &mut environment) {
                        break;
                    },
                    None => return,
                },
            }
        }
    }
}

/// Handles a command while disconnected; false to try connecting now.
fn offline(
    ui: &AppHandle<Desktop>,
    command: Command,
    selected: &mut Option<ThreadId>,
    target: &mut Option<String>,
) -> bool {
    match command {
        Command::Select(thread) => *selected = Some(thread),
        Command::ClearSelection => *selected = None,
        Command::Environment {
            environment,
            thread,
        } => {
            *selected = thread;
            *target = Some(environment);
            return false;
        }
        Command::Create { draft, params } => {
            ui.update(move |s, tx| {
                s.restore_first_message(draft, params);
                s.toast(tx, "Offline", None);
            });
        }
        Command::LoadDraft(id) => ui.update(move |s, _| s.draft_loaded(id)),
        Command::SendInput(send_input) => {
            ui.update(move |s, tx| {
                s.restore_message(send_input);
                s.toast(tx, "Offline", None);
            });
        }
        Command::SearchFiles { .. } | Command::ListBranches(_) => {}
        Command::Git(GitCommand::ThreadDiff { thread }) => {
            ui.update(move |s, _| {
                if s.selected == Some(thread) {
                    s.diff.apply_error(DiffScope::Thread, "Offline".into());
                }
            });
        }
        Command::Git(GitCommand::RunDiff {
            thread,
            run,
            turn,
            summary,
        }) => {
            ui.update(move |s, _| {
                if s.selected == Some(thread) {
                    if summary {
                        s.diff.clear_run_summary_loading(run);
                    } else {
                        s.diff
                            .apply_error(DiffScope::Run { run, turn }, "Offline".into());
                    }
                }
            });
        }
        Command::SearchThreads { request, .. } => {
            ui.update(move |s, _| s.palette.set_hits(request, Err("Offline".into())));
        }
        Command::LoadEarlier { thread, .. } => {
            ui.update(move |s, tx| s.loaded_earlier(thread, None, tx));
        }
        Command::Attach {
            scope,
            target,
            source,
        } => requests::attach(ui, scope, target, source),
        Command::Notify { title, body } => requests::notify(&title, body),
        Command::Reconnect => return false,
        _ => toast(ui, "Offline", "Server disconnected".into()),
    }
    true
}

/// Why a session ended.
enum Ended {
    /// The UI is gone.
    Quit,
    /// The connection failed.
    Lost(Error),
    Environment {
        target: String,
        thread: Option<ThreadId>,
    },
}

/// One connection's life: list, watch, and serve commands until it closes.
async fn session(
    ui: &AppHandle<Desktop>,
    connection: Connection,
    commands: &mut mpsc::UnboundedReceiver<Command>,
    selected: &mut Option<ThreadId>,
    first: &mut Option<std::sync::mpsc::Sender<()>>,
) -> Ended {
    let Connection {
        client,
        mut notifications,
        hello,
    } = connection;
    let mut session = Session {
        ui,
        client,
        environment: Arc::from(hello.environment.as_str()),
        watched: None,
        entries: Vec::new(),
        before: None,
    };
    let started = async {
        let ((), harnesses, environments) = tokio::try_join!(
            session.refresh_list(),
            session.client.list_harnesses(),
            session.client.request(&ListEnvironments {}),
        )?;
        if let Some(thread) = *selected {
            match session.switch(Some(thread)).await {
                Err(Error::Rpc {
                    code: NOT_FOUND, ..
                }) => *selected = None,
                result => result?,
            }
        }
        let watched = *selected;
        ui.update(move |s, tx| {
            s.status = Status::Connected {
                version: hello.version,
            };
            s.environment = Some(hello.environment);
            s.environments = environments.environments;
            s.harnesses = harnesses;
            if s.new_thread.is_none() {
                if let Some(thread) = watched {
                    s.show(thread);
                    tx.focus("draft", false);
                } else {
                    s.clear_selection();
                }
            }
            s.update_title(tx);
        });
        Ok(())
    };
    if let Err(e) = started.await {
        return Ended::Lost(e);
    }
    signal(first);
    loop {
        let result = tokio::select! {
            notification = notifications.recv() => match notification {
                Some(notification) => session.on_notification(notification).await,
                None => Err(Error::Closed),
            },
            command = commands.recv() => match command {
                Some(Command::Environment { environment, thread }) => {
                    session.client.close();
                    return Ended::Environment { target: environment, thread };
                }
                Some(command) => session.on_command(command, selected).await,
                None => return Ended::Quit,
            },
        };
        match result {
            Ok(()) => {}
            Err(Error::Closed) => return Ended::Lost(Error::Closed),
            Err(e) => toast(ui, "Request failed", e.to_string()),
        }
    }
}

struct Session<'a> {
    ui: &'a AppHandle<Desktop>,
    client: Client,
    environment: Arc<str>,
    watched: Option<ThreadId>,
    entries: Vec<Arc<Vec<Entry>>>,
    before: Option<EntryId>,
}

impl Session<'_> {
    async fn on_command(
        &mut self,
        command: Command,
        selected: &mut Option<ThreadId>,
    ) -> viz_client::Result<()> {
        match command {
            Command::Select(thread) => {
                self.select(thread).await?;
                *selected = Some(thread);
                Ok(())
            }
            Command::ClearSelection => {
                self.switch(None).await?;
                *selected = None;
                Ok(())
            }
            Command::Create { draft, params } => self.launch(draft, params).await,
            Command::CancelTask(task) => {
                self.client.request(&CancelTask { task: &task }).await?;
                self.refresh_tasks().await
            }
            Command::Environment { .. } => unreachable!("environment changes replace the session"),
            Command::LoadDraft(id) => {
                self.load_draft(id);
                Ok(())
            }
            Command::HarnessOptions { harness, model } => {
                self.harness_options(harness, model);
                Ok(())
            }
            Command::Update(thread, change) => self.update_thread(thread, &change).await,
            Command::Delete(thread) => self.client.delete_thread(thread).await,
            Command::Configure {
                thread,
                option,
                value,
            } => {
                self.client
                    .configure_thread(&ConfigureThread {
                        thread,
                        option: &option,
                        value: &value,
                    })
                    .await
            }
            Command::Abort(id) => self.client.stop_thread(id).await,
            Command::Project(change) => self.project(change).await,
            Command::ListBranches(project) => {
                self.list_branches(project);
                Ok(())
            }
            Command::Git(git) => self.on_git(git).await,
            Command::SearchFiles {
                thread,
                request,
                query,
            } => {
                self.search(thread, request, query);
                Ok(())
            }
            Command::SearchThreads { request, query } => {
                requests::search(self.ui, &self.client, request, query);
                Ok(())
            }
            Command::LoadEarlier { thread, before } => self.load_earlier(thread, before).await,
            Command::RemoveQueued { thread, run } => {
                self.client
                    .remove_queued_input(&RemoveQueuedInput { thread, run })
                    .await
            }
            Command::EditQueued { thread, run, text } => {
                self.update_queued(thread, run, Some(&text), None).await
            }
            Command::MoveQueued {
                thread,
                run,
                before,
            } => self.update_queued(thread, run, None, Some(before)).await,
            Command::SteerQueued(input) => self.client.request(&input).await.map(drop),
            Command::Notify { title, body } => {
                requests::notify(&title, body);
                Ok(())
            }
            Command::Attach {
                scope,
                target,
                source,
            } => {
                requests::attach(self.ui, scope, target, source);
                Ok(())
            }
            Command::SendInput(send_input) => self.send_input(send_input).await,
            Command::Reconnect => Ok(()),
        }
    }

    async fn select(&mut self, thread: ThreadId) -> viz_client::Result<()> {
        if let Err(error) = self.switch(Some(thread)).await {
            let previous = self.watched;
            let entries: Vec<Entry> = self
                .entries
                .iter()
                .flat_map(|page| page.iter().cloned())
                .collect();
            let before = self.before;
            self.ui.update(move |s, tx| {
                if s.selected == Some(thread) {
                    if let Some(previous) = previous {
                        s.show(previous);
                        s.apply_view(previous, &entries, true, before);
                    } else {
                        s.clear_selection();
                    }
                    s.update_title(tx);
                }
            });
            return Err(error);
        }
        Ok(())
    }

    async fn launch(&self, draft: u64, params: LaunchThread) -> viz_client::Result<()> {
        match self.client.launch_thread(&params).await {
            Ok(thread) => self.ui.update(move |s, tx| s.created(draft, thread, tx)),
            Err(error) => {
                self.ui
                    .update(move |s, _| s.restore_first_message(draft, params));
                return Err(error);
            }
        }
        Ok(())
    }

    async fn send_input(&self, send_input: SendInput) -> viz_client::Result<()> {
        let result = self.client.send(&send_input).await;
        if result.is_err() {
            self.ui.update(move |s, _| s.restore_message(send_input));
        }
        result.map(drop)
    }

    #[expect(
        clippy::option_option,
        reason = "mirrors wire UpdateQueuedInput::before patch semantics"
    )]
    async fn update_queued(
        &self,
        thread: ThreadId,
        run: RunId,
        text: Option<&str>,
        before: Option<Option<RunId>>,
    ) -> viz_client::Result<()> {
        self.client
            .request(&UpdateQueuedInput {
                thread,
                run,
                text,
                before,
            })
            .await
            .map(drop)
    }

    fn load_draft(&self, id: u64) {
        let ui = self.ui.clone();
        let client = self.client.clone();
        tokio::spawn(async move {
            let harnesses = client.list_harnesses().await;
            ui.update(move |s, tx| {
                if s.new_thread.as_ref().is_none_or(|thread| thread.id != id) {
                    return;
                }
                match harnesses {
                    Ok(harnesses) => s.harnesses = harnesses,
                    Err(error) => s.toast(tx, "Couldn’t load harnesses", Some(error.to_string())),
                }
                s.draft_loaded(id);
            });
        });
    }

    async fn update_thread(
        &self,
        thread: ThreadId,
        change: &ThreadChange,
    ) -> viz_client::Result<()> {
        let mut params = UpdateThread {
            thread,
            title: None,
            archived: None,
            pinned: None,
            unread: None,
            project: None,
        };
        match change {
            ThreadChange::Title(title) => params.title = Some(title),
            ThreadChange::Archive(archived) => params.archived = Some(*archived),
            ThreadChange::Pin(pinned) => params.pinned = Some(*pinned),
            ThreadChange::Unread(unread) => params.unread = Some(*unread),
            ThreadChange::Project(project) => params.project = Some(project.as_deref()),
        }
        self.client.update_thread(&params).await
    }

    async fn on_notification(&mut self, notification: Notification) -> viz_client::Result<()> {
        match notification {
            Notification::ThreadsChanged(_) => self.refresh_list().await,
            Notification::TasksChanged(_) => self.refresh_tasks().await,
            Notification::ThreadState { thread, state } => {
                self.ui.update(move |s, tx| s.set_state(thread, state, tx));
                Ok(())
            }
            Notification::ThreadView {
                thread,
                entries,
                reset,
                before,
            } => {
                if self.watched == Some(thread) {
                    if reset {
                        self.entries.clear();
                        self.before = before;
                    }
                    let entries = Arc::new(entries);
                    self.entries.push(Arc::clone(&entries));
                    self.ui
                        .update(move |s, _| s.apply_view(thread, &entries, reset, before));
                }
                Ok(())
            }
            Notification::Reply { .. } => Ok(()),
        }
    }

    async fn switch(&mut self, thread: Option<ThreadId>) -> viz_client::Result<()> {
        if thread == self.watched {
            return Ok(());
        }
        if let Some(id) = thread {
            self.client.watch(id).await?;
        }
        let previous = self.watched;
        self.watched = thread;
        self.entries.clear();
        self.before = None;
        if let Some(previous) = previous
            && let Err(error) = self.client.unwatch(previous).await
        {
            if matches!(error, Error::Closed) {
                return Err(error);
            }
            toast(self.ui, "Couldn’t release thread watch", error.to_string());
        }
        Ok(())
    }

    async fn refresh_tasks(&self) -> viz_client::Result<()> {
        let tasks = self.client.request(&ListTasks::default()).await?.tasks;
        let environment = Arc::clone(&self.environment);
        self.ui.update(move |s, _| {
            if s.environment
                .as_deref()
                .is_none_or(|active| active == environment.as_ref())
            {
                s.tasks = tasks;
            }
        });
        Ok(())
    }

    async fn refresh_list(&self) -> viz_client::Result<()> {
        let (mut threads, mut projects, ()) = tokio::try_join!(
            self.client.list_threads(),
            self.client.list_projects(),
            self.refresh_tasks(),
        )?;
        threads.sort_unstable_by_key(|thread| Reverse((thread.updated_at, thread.id)));
        projects.sort_unstable_by(|a, b| a.name.cmp(&b.name).then(a.path.cmp(&b.path)));
        let environment = Arc::clone(&self.environment);
        self.ui.update(move |s, tx| {
            if s.environment
                .as_deref()
                .is_none_or(|active| active == environment.as_ref())
            {
                s.projects = projects;
                s.set_threads(threads, tx);
            }
        });
        Ok(())
    }

    async fn project(&self, change: ProjectChange) -> viz_client::Result<()> {
        match &change {
            ProjectChange::Add(path) => {
                let project = self
                    .client
                    .create_project(&CreateProject { path, name: None })
                    .await?;
                self.ui.update(move |s, _| s.project_added(project));
                Ok(())
            }
            ProjectChange::Rename { id, name } => {
                self.client
                    .update_project(&UpdateProject {
                        project: id,
                        name: Some(name),
                    })
                    .await
            }
            ProjectChange::Remove(id) => {
                self.client
                    .remove_project(&RemoveProject { project: id })
                    .await
            }
        }
    }

    fn list_branches(&self, project: String) {
        let ui = self.ui.clone();
        let client = self.client.clone();
        let environment = Arc::clone(&self.environment);
        tokio::spawn(async move {
            let result = client
                .request(&ListGitBranches {
                    project: project.clone(),
                })
                .await;
            ui.update(move |s, _| {
                if s.environment.as_deref() == Some(environment.as_ref())
                    && let Ok(listed) = result
                {
                    s.branches.insert(project, listed.branches);
                }
            });
        });
    }

    async fn on_git(&self, command: GitCommand) -> viz_client::Result<()> {
        match command {
            GitCommand::ThreadDiff { thread } => self.load_thread_diff(thread),
            GitCommand::RunDiff {
                thread,
                run,
                turn,
                summary,
            } => self.load_run_diff(thread, run, turn, summary),
            GitCommand::RestoreRun { thread, run } => {
                self.client.request(&RestoreRun { thread, run }).await?;
                self.ui.update(move |s, tx| {
                    if s.selected == Some(thread) {
                        s.toast(tx, "Restored working tree", None);
                        if s.diff.open {
                            s.open_thread_diff(tx);
                        }
                    }
                });
            }
            GitCommand::RewindThread {
                thread,
                run,
                prompt_text,
            } => {
                self.client.request(&RewindThread { thread, run }).await?;
                self.ui
                    .update(move |s, tx| s.rewound(thread, &prompt_text, tx));
            }
            GitCommand::ForkThread {
                thread,
                run,
                boundary,
                worktree,
            } => {
                let launched = self
                    .client
                    .request(&ForkThread {
                        thread,
                        run,
                        boundary,
                        worktree: worktree.then_some(ForkWorktree { branch: None }),
                        title: None,
                    })
                    .await?;
                self.ui.update(move |s, tx| s.forked(launched.thread, tx));
            }
        }
        Ok(())
    }

    fn load_thread_diff(&self, thread: ThreadId) {
        let ui = self.ui.clone();
        let client = self.client.clone();
        tokio::spawn(async move {
            let result = client
                .request(&ThreadDiff {
                    thread,
                    summary: false,
                })
                .await;
            ui.update(move |s, tx| {
                if s.selected != Some(thread) {
                    return;
                }
                match result {
                    Ok(diff) => {
                        let files = diff
                            .files
                            .into_iter()
                            .map(|file| DiffFile {
                                path: file.path,
                                change: file.change,
                            })
                            .collect();
                        s.diff
                            .apply_diff(DiffScope::Thread, files, diff.patch.as_deref(), tx);
                    }
                    Err(error) => s.diff.apply_error(DiffScope::Thread, error.to_string()),
                }
            });
        });
    }

    fn load_run_diff(&self, thread: ThreadId, run: RunId, turn: usize, summary: bool) {
        let ui = self.ui.clone();
        let client = self.client.clone();
        tokio::spawn(async move {
            let result = client
                .request(&RunDiff {
                    thread,
                    run,
                    summary,
                })
                .await;
            ui.update(move |s, tx| {
                if s.selected != Some(thread) {
                    return;
                }
                let scope = DiffScope::Run { run, turn };
                match result {
                    Ok(diff) => {
                        let files: Vec<DiffFile> = diff
                            .files
                            .into_iter()
                            .map(|file| DiffFile {
                                path: file.path,
                                change: file.change,
                            })
                            .collect();
                        if summary {
                            s.diff.apply_run_summary(run, files);
                        } else {
                            s.diff.apply_diff(scope, files, diff.patch.as_deref(), tx);
                        }
                    }
                    Err(error) => {
                        if summary {
                            s.diff.clear_run_summary_loading(run);
                            s.toast(tx, "Couldn’t load changed files", Some(error.to_string()));
                        } else {
                            s.diff.apply_error(scope, error.to_string());
                        }
                    }
                }
            });
        });
    }

    fn search(&self, thread: ThreadId, request: u64, query: String) {
        let ui = self.ui.clone();
        let client = self.client.clone();
        tokio::spawn(async move {
            let result = client
                .search_files(&SearchFiles {
                    thread,
                    query: &query,
                    limit: Some(24),
                })
                .await;
            ui.update(move |s, _| {
                if s.selected == Some(thread) {
                    s.completion.file_result(request, result);
                }
            });
        });
    }

    fn harness_options(&self, harness: String, model: String) {
        let ui = self.ui.clone();
        let client = self.client.clone();
        let environment = Arc::clone(&self.environment);
        tokio::spawn(async move {
            let result = client
                .harness_options(&HarnessOptions {
                    harness: harness.clone(),
                    model,
                })
                .await;
            ui.update(move |s, tx| {
                if s.environment.as_deref() != Some(environment.as_ref()) {
                    return;
                }
                match result {
                    Ok(result) => s.set_harness_options(&harness, result.options),
                    Err(error) => {
                        s.toast(tx, "Couldn’t load model options", Some(error.to_string()));
                    }
                }
            });
        });
    }
}
