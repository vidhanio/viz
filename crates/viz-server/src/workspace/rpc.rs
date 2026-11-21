use std::{num::NonZeroU64, path::Path, sync::Arc, time::Duration};

use futures_util::{StreamExt as _, stream::FuturesUnordered};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Value, json};
use viz_client::protocol::{
    DeleteThread, HarnessOptions, LaunchThread, Launched, Method, RemoveProject, RunId, SendInput,
    SendMode, Sent, SteerQueuedInput, ThreadId,
};

use super::{
    Workspace, attachments,
    runs::{self, NewRun, Place},
    runtime,
    sql::{exec, row, rows},
    store::{self, NewThread, SavedSession},
};
use crate::{
    acp::{self, Selection},
    wire::{Attachment, Harness, OptionValue, Project, RpcError, decode, encode},
};

type Id = NonZeroU64;

impl Workspace {
    pub(crate) async fn call(
        self: &Arc<Self>,
        method: &str,
        params: Value,
    ) -> Result<Value, RpcError> {
        match method {
            "threads.list" | "thread.entries" | "thread.state" | "threads.search"
            | "files.search" => self.query_thread(method, params).await,
            "projects.list"
            | "project.read"
            | "project.create"
            | "project.update"
            | RemoveProject::NAME => self.project_call(method, params).await,
            LaunchThread::NAME => encode(self.launch(decode(params)?).await?),
            SendInput::NAME => {
                let input: SendInput = decode(params)?;
                if input.origin.is_some() {
                    return Err(RpcError::invalid("origins require a connector WebSocket"));
                }
                encode(self.send(input).await?)
            }
            "thread.wait" => {
                let Wait {
                    thread,
                    run,
                    timeout_ms,
                } = decode(params)?;
                let timeout = timeout_ms.unwrap_or(30_000);
                if timeout > 120_000 {
                    return Err(RpcError::invalid("timeoutMs must not exceed 120000"));
                }
                encode(
                    self.wait_run(
                        thread.get(),
                        run.map(Id::get),
                        Duration::from_millis(timeout),
                    )
                    .await?,
                )
            }
            "thread.stop" => {
                let ThreadRun { thread, run } = decode(params)?;
                self.stop(thread.get(), run.map(Id::get)).await?;
                Ok(json!({}))
            }
            "thread.update" => {
                self.update(decode(params)?).await?;
                Ok(json!({}))
            }
            "thread.configure" => {
                let Configure {
                    thread,
                    option,
                    value,
                } = decode(params)?;
                self.configure(thread.get(), &option, value).await?;
                Ok(json!({}))
            }
            DeleteThread::NAME => {
                let input: DeleteThread = decode(params)?;
                self.delete(input.thread.0).await?;
                Ok(json!({}))
            }
            "queue.remove" => {
                let QueueRun { thread, run } = decode(params)?;
                let mut db = self.db.lock().await;
                store::require(&mut *db, thread.get()).await?;
                runs::remove_queued(&mut *db, thread.get(), run.get()).await?;
                drop(db);
                self.settled(thread.get());
                Ok(json!({}))
            }
            "queue.update" => self.update_queue(decode(params)?).await,
            SteerQueuedInput::NAME => self.steer_queue(decode(params)?).await,
            "harnesses.list" => Ok(json!({"harnesses": self.list_harnesses().await})),
            HarnessOptions::NAME => {
                let HarnessOptions { harness, model } = decode(params)?;
                Ok(json!({"options": self.harness_options(&harness, Some(&model)).await?}))
            }
            "task.receive" => encode(self.receive(decode(params)?).await?),
            "task.retract" => encode(self.retract(decode(params)?).await?),
            _ => Err(RpcError::new(-32601, format!("unknown method {method}"))),
        }
    }

    async fn update_queue(&self, input: UpdateQueue) -> Result<Value, RpcError> {
        let UpdateQueue {
            thread,
            run,
            before,
            text,
        } = input;
        let mut db = self.db.lock().await;
        let mut tx = db.transaction().await?;
        store::require(&mut tx, thread.get()).await?;
        runs::update_queue(&mut tx, thread.get(), run.get(), text.as_deref(), before).await?;
        tx.commit().await?;
        drop(db);
        self.changed(Some(thread.get()));
        Ok(json!({}))
    }

    async fn steer_queue(self: &Arc<Self>, input: SteerQueuedInput) -> Result<Value, RpcError> {
        let thread = input.thread.0;
        let mut db = self.db.lock().await;
        let mut tx = db.transaction().await?;
        store::require(&mut tx, thread).await?;
        let active = runs::active(&mut tx, thread).await?;
        runs::promote_queued(&mut tx, thread, input.run.0).await?;
        tx.commit().await?;
        if let Some(active) = active {
            self.steer_active(thread, active);
        }
        self.kick(thread, false);
        drop(db);
        self.changed(Some(thread));
        Ok(json!({}))
    }

    async fn query_thread(&self, method: &str, params: Value) -> Result<Value, RpcError> {
        match method {
            "threads.list" => {
                let ListThreads { project, archived } = decode(params)?;
                let threads = store::list_threads(
                    &mut *self.db.lock().await,
                    archived,
                    project.as_deref(),
                    self,
                )
                .await?;
                Ok(json!({"threads":threads}))
            }
            "thread.entries" => {
                let Entries {
                    thread,
                    before,
                    limit,
                } = decode(params)?;
                encode(
                    self.entries(thread.get(), before.map(Id::get), page_limit(limit, 100)?)
                        .await?,
                )
            }
            "thread.state" => {
                let Thread { thread } = decode(params)?;
                Ok(json!({"state": self.state(thread.get()).await?}))
            }
            "threads.search" => {
                let Search { query, limit } = decode(params)?;
                let hits =
                    store::search(&mut *self.db.lock().await, &query, page_limit(limit, 50)?)
                        .await?;
                Ok(json!({"hits":hits}))
            }
            "files.search" => {
                let FileSearch {
                    thread,
                    query,
                    limit,
                } = decode(params)?;
                let cwd = row(
                    &mut *self.db.lock().await,
                    "SELECT cwd FROM threads WHERE id = ?1",
                    [thread.get().into()],
                )
                .await?
                .ok_or_else(|| store::missing(thread.get()))?
                .str(0)?;
                Ok(json!({"files": self.files.search(&cwd, &query, page_limit(limit, 50)?).await}))
            }
            _ => Err(RpcError::new(
                -32601,
                format!("unknown thread query {method}"),
            )),
        }
    }

    async fn project_call(&self, method: &str, params: Value) -> Result<Value, RpcError> {
        match method {
            "projects.list" => {
                let projects = store::projects(&mut *self.db.lock().await).await?;
                Ok(json!({"projects":projects}))
            }
            "project.read" => {
                let ProjectKey { project } = decode(params)?;
                let project = store::project(&mut *self.db.lock().await, &project).await?;
                Ok(json!({"project":project}))
            }
            "project.create" => {
                let CreateProject { path, name } = decode(params)?;
                Ok(json!({"project": self.create_project(&path, name).await?}))
            }
            "project.update" => {
                let UpdateProject { project, name } = decode(params)?;
                let mut db = self.db.lock().await;
                store::project(&mut *db, &project).await?;
                if let Some(name) = name {
                    if name.is_empty() {
                        return Err(RpcError::invalid("project name must not be empty"));
                    }
                    exec(
                        &mut *db,
                        "UPDATE projects SET name = ?2 WHERE path = ?1",
                        [project.into(), name.into()],
                    )
                    .await?;
                    drop(db);
                    self.changed(None);
                }
                Ok(json!({}))
            }
            RemoveProject::NAME => {
                let ProjectKey { project } = decode(params)?;
                self.remove_project(&project).await?;
                Ok(json!({}))
            }
            _ => Err(RpcError::new(
                -32601,
                format!("unknown project method {method}"),
            )),
        }
    }

    async fn launch(self: &Arc<Self>, args: LaunchThread) -> Result<Launched, RpcError> {
        let mut db = self.db.lock().await;
        let mut tx = db.transaction().await?;
        validate_request_id(args.request_id.as_deref())?;
        if let Some(request_id) = &args.request_id
            && let Some(found) = row(
                &mut tx,
                "SELECT thread, run FROM launch_requests WHERE request_id = ?1",
                [request_id.as_str().into()],
            )
            .await?
        {
            return Ok(Launched {
                thread: ThreadId(found.u64(0)?),
                run: found.opt_u64(1)?.map(RunId),
            });
        }
        if args.title.as_deref() == Some("") {
            return Err(RpcError::invalid("thread title must not be empty"));
        }
        let (harness, _) = self.harness(args.harness.as_deref())?;
        let cwd = self.cwd_for(&mut tx, args.project.as_deref()).await?;
        let attachments = attachments::decode(args.attachments)?;
        let session = SavedSession {
            desired: args.options,
            selection: selection(args.model, args.thinking),
            ..SavedSession::default()
        };
        let thread = store::create_thread(
            &mut tx,
            &NewThread {
                harness,
                cwd: store::path_text(&cwd)?,
                project: args.project.as_deref(),
                parent: None,
                parent_environment: None,
                title: args.title.as_deref(),
                session,
            },
        )
        .await?;
        let run = if args.text.is_some() || !attachments.is_empty() {
            let attachments = attachments::save(&mut tx, thread, attachments).await?;
            Some(
                runs::enqueue(
                    &mut tx,
                    thread,
                    NewRun {
                        attachments: &attachments,
                        ..NewRun::new(args.text.as_deref().unwrap_or_default())
                    },
                )
                .await?,
            )
        } else {
            None
        };
        if let Some(request_id) = args.request_id {
            exec(
                &mut tx,
                "INSERT INTO launch_requests (request_id, thread, run) VALUES (?1, ?2, ?3)",
                [request_id.into(), thread.into(), run.into()],
            )
            .await?;
        }
        tx.commit().await?;
        self.kick(thread, run.is_none());
        drop(db);
        self.relist(thread);
        Ok(Launched {
            thread: ThreadId(thread),
            run: run.map(RunId),
        })
    }

    pub(crate) async fn send(self: &Arc<Self>, args: SendInput) -> Result<Sent, RpcError> {
        let thread = args.thread.0;
        let mut db = self.db.lock().await;
        let mut tx = db.transaction().await?;
        store::require(&mut tx, thread).await?;
        validate_request_id(args.request_id.as_deref())?;
        if let Some(request_id) = &args.request_id
            && let Some(run) = runs::by_request(&mut tx, thread, request_id).await?
        {
            return Ok(Sent { run: RunId(run) });
        }
        let active = if args.mode == Some(SendMode::Steer) {
            runs::active(&mut tx, thread).await?
        } else {
            None
        };
        let place = if active.is_some() {
            Place::Front
        } else {
            Place::End
        };
        let attachments =
            attachments::save(&mut tx, thread, attachments::decode(args.attachments)?).await?;
        let run = runs::enqueue(
            &mut tx,
            thread,
            NewRun {
                origin: args.origin.as_ref(),
                attachments: &attachments,
                request_id: args.request_id.as_deref(),
                place,
                ..NewRun::new(&args.text)
            },
        )
        .await?;
        tx.commit().await?;
        if let Some(active) = active {
            self.steer_active(thread, active);
        }
        self.kick(thread, false);
        drop(db);
        self.relist(thread);
        Ok(Sent { run: RunId(run) })
    }

    fn cancel_active(&self, thread: u64, active: u64) {
        let state = self
            .transient
            .lock()
            .expect("workspace runtime mutex poisoned");
        if let Some(driver) = state.threads.get(&thread) {
            driver.cancel(active);
        }
    }

    fn steer_active(&self, thread: u64, active: u64) {
        let state = self
            .transient
            .lock()
            .expect("workspace runtime mutex poisoned");
        if let Some(driver) = state.threads.get(&thread) {
            driver.steer(active);
        }
    }

    async fn stop(&self, thread: u64, requested: Option<u64>) -> Result<(), RpcError> {
        let active = {
            let mut db = self.db.lock().await;
            store::require(&mut *db, thread).await?;
            if let Some(run) = requested
                && runs::status(&mut *db, thread, run).await?.is_none()
            {
                return Err(RpcError::not_found(format!(
                    "no run {run} in thread {thread}"
                )));
            }
            runs::cancel_queued(&mut *db, thread, requested).await?;
            let active = runs::active(&mut *db, thread)
                .await?
                .filter(|&run| requested.is_none_or(|requested| requested == run));
            if let Some(active) = active {
                self.cancel_active(thread, active);
            }
            drop(db);
            active
        };
        self.settled(thread);
        if let Some(active) = active {
            let result = self
                .wait_run(thread, Some(active), Duration::from_secs(120))
                .await?;
            if result.timed_out {
                return Err(RpcError::unavailable(
                    "ACP cancellation is still in progress",
                ));
            }
        }
        Ok(())
    }

    async fn update(&self, args: UpdateThread) -> Result<(), RpcError> {
        let thread = args.thread.get();
        let mut db = self.db.lock().await;
        store::require(&mut *db, thread).await?;
        if args.title.as_deref() == Some("") {
            return Err(RpcError::invalid("thread title must not be empty"));
        }
        if let Patch::Set(project) = &args.project {
            store::project(&mut *db, project).await?;
        }
        exec(&mut *db, "UPDATE threads SET title = coalesce(?2, title), user_title = CASE WHEN ?2 IS NULL THEN user_title ELSE 1 END,
            archived = coalesce(?3, archived), pinned = coalesce(?4, pinned), unread = coalesce(?5, unread),
            project = CASE WHEN ?6 THEN ?7 ELSE project END WHERE id = ?1", [
            thread.into(), args.title.into(), args.archived.into(), args.pinned.into(), args.unread.into(),
            (!matches!(args.project, Patch::Keep)).into(), match args.project {
                Patch::Set(project) => Some(project),
                Patch::Keep | Patch::Clear => None,
            }.into(),
        ]).await?;
        drop(db);
        self.relist(thread);
        Ok(())
    }

    async fn delete(&self, thread: u64) -> Result<(), RpcError> {
        let driver = {
            let mut db = self.db.lock().await;
            let count = exec(
                &mut *db,
                "DELETE FROM threads WHERE id = ?1",
                [thread.into()],
            )
            .await?;
            if count == 0 {
                return Err(store::missing(thread));
            }
            let mut state = self
                .transient
                .lock()
                .expect("workspace runtime mutex poisoned");
            state.live.remove(&thread);
            state.watches.remove(&thread);
            let driver = state.threads.remove(&thread);
            drop(state);
            drop(db);
            driver
        };
        self.settled(thread);
        if let Some(driver) = driver {
            driver.lifetime.cancel();
            driver.session.lock().await.take();
        }
        match tokio::fs::remove_dir_all(self.attachment_dir(thread)).await {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                tracing::warn!(thread, %error, "removing materialized thread attachments");
            }
        }
        Ok(())
    }

    async fn create_project(&self, path: &str, name: Option<String>) -> Result<Project, RpcError> {
        if path.is_empty() || name.as_deref() == Some("") {
            return Err(RpcError::invalid("project path and name must not be empty"));
        }
        let canonical = store::directory(Path::new(path)).await?;
        let path = store::path_text(&canonical)?;
        let name = name.unwrap_or_else(|| {
            canonical
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(path)
                .to_owned()
        });
        let mut db = self.db.lock().await;
        let created_at = store::now();
        let count = exec(&mut *db, "INSERT INTO projects (path, name, created_at) VALUES (?1, ?2, ?3) ON CONFLICT DO NOTHING",
            [path.into(), name.as_str().into(), created_at.into()]).await?;
        drop(db);
        if count == 0 {
            return Err(RpcError::invalid(format!("project already exists: {path}")));
        }
        self.changed(None);
        Ok(Project {
            path: path.to_owned(),
            name,
            created_at,
        })
    }

    async fn remove_project(&self, project: &str) -> Result<(), RpcError> {
        let mut db = self.db.lock().await;
        let mut tx = db.transaction().await?;
        let threads = rows(
            &mut tx,
            "SELECT id FROM threads WHERE project = ?1",
            [project.into()],
        )
        .await?;
        let count = exec(
            &mut tx,
            "DELETE FROM projects WHERE path = ?1",
            [project.into()],
        )
        .await?;
        if count == 0 {
            return Err(RpcError::not_found(format!("no project {project}")));
        }
        tx.commit().await?;
        drop(db);
        self.changed(None);
        for thread in threads {
            self.changed(Some(thread.u64(0)?));
        }
        Ok(())
    }

    async fn list_harnesses(&self) -> Vec<Harness> {
        let pending = self
            .harnesses
            .keys()
            .map(|name| self.harness_summary(name))
            .collect::<FuturesUnordered<_>>();
        let mut harnesses: Vec<_> = pending.collect().await;
        harnesses.sort_unstable_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
        harnesses
    }

    async fn harness_summary(&self, name: &str) -> Harness {
        let options = self
            .harness_options(name, None)
            .await
            .unwrap_or_else(|error| {
                tracing::warn!(harness = %name, %error, "ACP catalog unavailable");
                Vec::new()
            });
        Harness {
            id: name.to_owned(),
            name: self.harness_name(name).to_owned(),
            default: self.default_harness.as_deref() == Some(name),
            options,
        }
    }

    async fn harness_options(
        &self,
        name: &str,
        model: Option<&str>,
    ) -> Result<Vec<crate::wire::ConfigOption>, RpcError> {
        let (_, config) = self.harness(Some(name))?;
        let stored = {
            let mut db = self.db.lock().await;
            runtime::catalog(&mut *db, name).await?
        };
        let (options, updated) =
            acp::catalog(config, self.default_cwd(), stored.as_ref(), model).await?;
        if let Some(updated) = updated {
            runtime::save_catalog(&mut *self.db.lock().await, name, &updated).await?;
        }
        Ok(options)
    }

    async fn receive(self: &Arc<Self>, mut args: ReceiveTask) -> Result<Received, RpcError> {
        let mut db = self.db.lock().await;
        let mut tx = db.transaction().await?;
        let key = [args.origin_environment.as_str(), args.task.as_str()];
        if let Some(found) = receipt(&mut tx, key).await? {
            return Ok(found);
        }
        if row(
            &mut tx,
            "SELECT 1 FROM task_retractions WHERE environment = ?1 AND task = ?2",
            key.map(Into::into),
        )
        .await?
        .is_some()
        {
            return Err(RpcError::invalid("task was cancelled before admission"));
        }
        if args.text.is_empty() {
            return Err(RpcError::invalid("task text must not be empty"));
        }
        let decoded = attachments::decode(std::mem::take(&mut args.attachments))?;
        let thread = self.task_thread(&mut tx, &args).await?;
        let attachments = attachments::save(&mut tx, thread, decoded).await?;
        let run = runs::enqueue(
            &mut tx,
            thread,
            NewRun {
                attachments: &attachments,
                ..NewRun::new(&args.text)
            },
        )
        .await?;
        exec(
            &mut tx,
            "INSERT INTO task_receipts (environment, task, thread, run) VALUES (?1, ?2, ?3, ?4)",
            [
                args.origin_environment.into(),
                args.task.into(),
                thread.into(),
                run.into(),
            ],
        )
        .await?;
        tx.commit().await?;
        self.kick(thread, false);
        drop(db);
        self.relist(thread);
        Ok(Received { thread, run })
    }

    async fn task_thread(
        &self,
        db: &mut dyn toasty::Executor,
        args: &ReceiveTask,
    ) -> Result<u64, RpcError> {
        if let Some(thread) = args.thread {
            let binding = store::binding(db, thread.get()).await?;
            if binding.parent.map(|id| id.0) != Some(args.parent.get())
                || binding.parent_environment.as_deref() != Some(args.origin_environment.as_str())
            {
                return Err(RpcError::invalid(
                    "target thread is not owned by this task parent",
                ));
            }
            if args
                .target
                .harness
                .as_ref()
                .is_some_and(|requested| *requested != binding.harness)
                || args
                    .target
                    .project
                    .as_ref()
                    .is_some_and(|requested| Some(requested) != binding.project.as_ref())
                || args.target.model.is_some()
                || args.target.thinking.is_some()
            {
                return Err(RpcError::invalid(
                    "existing task thread cannot change its harness, project, model, or thinking",
                ));
            }
            return Ok(thread.get());
        }
        let inherited = if args.origin_environment == self.environment {
            Some(store::binding(db, args.parent.get()).await?)
        } else {
            None
        };
        let project = args
            .target
            .project
            .as_deref()
            .or_else(|| inherited.as_ref()?.project.as_deref());
        let selected = args
            .target
            .harness
            .as_deref()
            .or_else(|| inherited.as_ref().map(|parent| parent.harness.as_str()));
        let (harness, _) = self.harness(selected)?;
        let resolved;
        let cwd = if args.target.project.is_none()
            && let Some(parent) = &inherited
        {
            parent.cwd.as_str()
        } else {
            resolved = self.cwd_for(db, args.target.project.as_deref()).await?;
            store::path_text(&resolved)?
        };
        let session = SavedSession {
            selection: selection(args.target.model.clone(), args.target.thinking.clone()),
            ..SavedSession::default()
        };
        store::create_thread(
            db,
            &NewThread {
                harness,
                cwd,
                project,
                parent: Some(args.parent.get()),
                parent_environment: Some(&args.origin_environment),
                title: None,
                session,
            },
        )
        .await
    }

    async fn retract(&self, args: TaskKey) -> Result<Option<Received>, RpcError> {
        let mut db = self.db.lock().await;
        let mut tx = db.transaction().await?;
        if let Some(found) = receipt(&mut tx, [&args.origin_environment, &args.task]).await? {
            return Ok(Some(found));
        }
        exec(&mut tx, "INSERT INTO task_retractions (environment, task) VALUES (?1, ?2) ON CONFLICT DO NOTHING",
            [args.origin_environment.into(), args.task.into()]).await?;
        tx.commit().await?;
        drop(db);
        Ok(None)
    }
}

async fn receipt(
    tx: &mut dyn toasty::Executor,
    [environment, task]: [&str; 2],
) -> Result<Option<Received>, RpcError> {
    row(
        tx,
        "SELECT thread, run FROM task_receipts WHERE environment = ?1 AND task = ?2",
        [environment.into(), task.into()],
    )
    .await?
    .map(|row| {
        Ok(Received {
            thread: row.u64(0)?,
            run: row.u64(1)?,
        })
    })
    .transpose()
}

fn validate_request_id(request_id: Option<&str>) -> Result<(), RpcError> {
    if request_id.is_some_and(|id| id.is_empty() || id.chars().count() > 256) {
        return Err(RpcError::invalid(
            "requestId must contain 1 to 256 characters",
        ));
    }
    Ok(())
}

fn page_limit(limit: Option<usize>, default: usize) -> Result<usize, RpcError> {
    match limit {
        Some(1..=200) | None => Ok(limit.unwrap_or(default)),
        _ => Err(RpcError::invalid("limit must be between 1 and 200")),
    }
}

#[derive(Debug, Default)]
pub(super) enum Patch<T> {
    #[default]
    Keep,
    Clear,
    Set(T),
}

fn nullable<'de, D: Deserializer<'de>, T: Deserialize<'de>>(de: D) -> Result<Patch<T>, D::Error> {
    Option::deserialize(de).map(|value| value.map_or(Patch::Clear, Patch::Set))
}

#[derive(Deserialize)]
struct ListThreads {
    archived: Option<bool>,
    project: Option<String>,
}

#[derive(Deserialize)]
struct Thread {
    thread: Id,
}

#[derive(Deserialize)]
struct ThreadRun {
    thread: Id,
    run: Option<Id>,
}

#[derive(Deserialize)]
struct QueueRun {
    thread: Id,
    run: Id,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Wait {
    thread: Id,
    run: Option<Id>,
    timeout_ms: Option<u64>,
}

#[derive(Deserialize)]
struct Entries {
    thread: Id,
    before: Option<Id>,
    limit: Option<usize>,
}

#[derive(Deserialize)]
struct Search {
    query: String,
    limit: Option<usize>,
}

#[derive(Deserialize)]
struct FileSearch {
    thread: Id,
    query: String,
    limit: Option<usize>,
}

#[derive(Deserialize)]
struct Configure {
    thread: Id,
    option: String,
    value: OptionValue,
}

#[derive(Deserialize)]
struct ProjectKey {
    project: String,
}

#[derive(Deserialize)]
struct CreateProject {
    path: String,
    name: Option<String>,
}

#[derive(Deserialize)]
struct UpdateProject {
    project: String,
    name: Option<String>,
}

#[derive(Deserialize)]
struct UpdateThread {
    thread: Id,
    title: Option<String>,
    archived: Option<bool>,
    pinned: Option<bool>,
    unread: Option<bool>,
    #[serde(default, deserialize_with = "nullable")]
    project: Patch<String>,
}

#[derive(Deserialize)]
struct UpdateQueue {
    thread: Id,
    run: Id,
    text: Option<String>,
    #[serde(default, deserialize_with = "nullable")]
    before: Patch<Id>,
}

#[derive(Deserialize)]
struct Target {
    project: Option<String>,
    harness: Option<String>,
    model: Option<String>,
    thinking: Option<String>,
}

fn selection(model: Option<String>, thinking: Option<String>) -> Option<Selection> {
    (model.is_some() || thinking.is_some()).then_some(Selection { model, thinking })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReceiveTask {
    task: String,
    origin_environment: String,
    parent: Id,
    thread: Option<Id>,
    text: String,
    #[serde(flatten)]
    target: Target,
    #[serde(default)]
    attachments: Vec<Attachment>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TaskKey {
    origin_environment: String,
    task: String,
}

#[derive(Serialize)]
struct Received {
    thread: u64,
    run: u64,
}
