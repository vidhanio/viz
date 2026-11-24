//! Approvals and questions from harnesses, and thread access modes.
//!
//! Every session forwards its requests, tagged with its thread, to one task
//! that holds the workspace. Under the thread's access mode a request is
//! either answered at once or recorded as pending and held until a client or
//! the delegating agent answers it, or until it is withdrawn: by the run
//! stopping, steering or ending, by the harness, or by a restart. A pending
//! request's waiter lives in memory only; the restart withdrawal settles the
//! records left behind.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use tokio::sync::{mpsc, oneshot};
use viz_acp::{ClientEvents, ClientRequest, LocalIo, PermissionPolicy, TerminalAccess};
use viz_core::{
    lifecycle,
    requests::{self, Admission, Asker, Command},
};
use viz_protocol::{
    AccessMode, AnswerRequest, AnsweredBy, OptionValue, RequestAnswer, RequestBody, RequestId,
    RequestState, RunId, RunRequest, SetProjectAccess, ThreadId, Withdrawal,
};

use super::{Workspace, sessions, sql, store};
use crate::error::Error;

type Incoming = (ThreadId, ClientRequest);

pub(super) struct Requests {
    incoming: mpsc::UnboundedSender<Incoming>,
    receiver: Mutex<Option<mpsc::UnboundedReceiver<Incoming>>>,
    waiters: Mutex<HashMap<RequestId, Waiter>>,
}

struct Waiter {
    run: RunId,
    answer: oneshot::Sender<RequestAnswer>,
}

impl Default for Requests {
    fn default() -> Self {
        let (incoming, receiver) = mpsc::unbounded_channel();
        Self {
            incoming,
            receiver: Mutex::new(Some(receiver)),
            waiters: Mutex::default(),
        }
    }
}

impl Requests {
    fn waiters(&self) -> std::sync::MutexGuard<'_, HashMap<RequestId, Waiter>> {
        self.waiters.lock().expect("request waiters poisoned")
    }

    /// Lets go of a run's waiters: the harness sees each request cancelled.
    pub(super) fn release_run(&self, run: RunId) {
        self.waiters().retain(|_, waiter| waiter.run != run);
    }
}

enum Admitted {
    Answer(RequestAnswer),
    Wait(RequestId, oneshot::Receiver<RequestAnswer>),
    Refuse,
}

enum Waited {
    Answered(RequestAnswer),
    Dropped,
    HarnessGone,
}

impl Workspace {
    /// The client services a thread's session uses: every request comes to
    /// this workspace, which applies the thread's access mode.
    pub(super) fn client_events(&self, thread: ThreadId) -> ClientEvents {
        let (sender, mut receiver) = mpsc::unbounded_channel();
        let incoming = self.requests.incoming.clone();
        tokio::spawn(async move {
            while let Some(request) = receiver.recv().await {
                if incoming.send((thread, request)).is_err() {
                    break;
                }
            }
        });
        ClientEvents {
            permissions: PermissionPolicy::Ask,
            requests: Some(sender),
            local_io: LocalIo {
                terminals: TerminalAccess::Local,
                ..LocalIo::default()
            },
            ..ClientEvents::default()
        }
    }

    /// Serves forwarded requests until the workspace closes.
    pub(super) async fn serve_requests(self: Arc<Self>) {
        let receiver = self
            .requests
            .receiver
            .lock()
            .expect("request receiver poisoned")
            .take();
        let Some(mut receiver) = receiver else {
            return;
        };
        loop {
            tokio::select! {
                () = self.closing.cancelled() => break,
                next = receiver.recv() => {
                    let Some((thread, request)) = next else { break };
                    let workspace = Arc::clone(&self);
                    tokio::spawn(async move { workspace.serve(thread, request).await });
                }
            }
        }
    }

    async fn serve(&self, thread: ThreadId, request: ClientRequest) {
        let ClientRequest::Interaction {
            body, mut reply, ..
        } = request
        else {
            return;
        };
        let admitted = match self.admit(thread, body).await {
            Ok(admitted) => admitted,
            Err(error) => {
                tracing::error!(%thread, %error, "recording a harness request");
                return;
            }
        };
        let (id, answer) = match admitted {
            Admitted::Answer(answer) => {
                let _ = reply.send(answer);
                return;
            }
            Admitted::Refuse => return,
            Admitted::Wait(id, answer) => (id, answer),
        };
        let waited = tokio::select! {
            answer = answer => answer.map_or(Waited::Dropped, Waited::Answered),
            () = reply.closed() => Waited::HarnessGone,
            () = self.closing.cancelled() => Waited::Dropped,
        };
        match waited {
            Waited::Answered(answer) => {
                let _ = reply.send(answer);
            }
            Waited::HarnessGone => {
                self.requests.waiters().remove(&id);
                if let Err(error) = self.withdraw_one(thread, id, Withdrawal::Harness).await {
                    tracing::error!(%thread, %error, "withdrawing a harness request");
                }
            }
            Waited::Dropped => {}
        }
    }

    /// Answers at once under full access; otherwise records the request for
    /// the thread's active run. Requests outside a run are refused.
    async fn admit(&self, thread: ThreadId, body: RequestBody) -> Result<Admitted, Error> {
        let Some(run) = self
            .workers
            .get(thread)
            .and_then(|driver| driver.active_run())
        else {
            return Ok(Admitted::Refuse);
        };
        let updates = Arc::clone(&self.updates);
        let requests = Arc::clone(&self.requests);
        let admitted = self
            .db
            .write(move |conn| {
                let options = store::binding(conn, thread)?.session.options;
                let access = store::access(conn, thread, &options)?;
                if let Admission::Answer(answer) = requests::admit(access.current.kind, &body) {
                    return Ok(Admitted::Answer(answer));
                }
                let watched = updates.is_watched(thread);
                let tx = sql::immediate(conn)?;
                let request = store::insert_request(&tx, thread, run, &body)?;
                store::touch(&tx, thread, |current| {
                    lifecycle::on_request(current, watched)
                })?;
                tx.commit()?;
                let (sender, receiver) = oneshot::channel();
                requests.waiters().insert(
                    request.id,
                    Waiter {
                        run,
                        answer: sender,
                    },
                );
                Ok(Admitted::Wait(request.id, receiver))
            })
            .await?;
        if matches!(admitted, Admitted::Wait(_, _)) {
            self.relist(thread);
        }
        Ok(admitted)
    }

    /// Resolves a pending request even if the RPC caller disconnects while
    /// the decision is being saved.
    pub(crate) async fn answer(
        self: &Arc<Self>,
        input: AnswerRequest,
        by: AnsweredBy,
    ) -> Result<RunRequest, Error> {
        let workspace = Arc::clone(self);
        self.jobs
            .spawn(async move { workspace.apply_answer(input, by).await })
            .await
            .map_err(|error| Error::unavailable(format!("answering request: {error}")))?
    }

    async fn apply_answer(
        &self,
        input: AnswerRequest,
        by: AnsweredBy,
    ) -> Result<RunRequest, Error> {
        let AnswerRequest {
            thread,
            request: id,
            answer,
        } = input;
        let requests = Arc::clone(&self.requests);
        let (request, delivered) = self
            .db
            .write(move |conn| {
                let mut request = store::request(conn, thread, id)?.ok_or_else(|| {
                    Error::not_found(format!("no request {id} in thread {thread}"))
                })?;
                let binding = store::binding(conn, thread)?;
                let asker = Asker {
                    parent: binding
                        .parent
                        .filter(|_| binding.parent_environment.is_none()),
                };
                request.state = requests::decide(
                    &request.state,
                    &request.body,
                    asker,
                    Command::Answer {
                        by,
                        answer: answer.clone(),
                    },
                    store::now(),
                )?;
                let waiting = requests
                    .waiters()
                    .get(&id)
                    .is_some_and(|waiter| !waiter.answer.is_closed());
                if waiting {
                    store::save_request(conn, &request)?;
                }
                let delivered = requests
                    .waiters()
                    .remove(&id)
                    .is_some_and(|waiter| waiting && waiter.answer.send(answer).is_ok());
                if !delivered {
                    request.state = RequestState::Withdrawn {
                        at: store::now(),
                        reason: Withdrawal::Harness,
                    };
                    store::save_request(conn, &request)?;
                }
                Ok((request, delivered))
            })
            .await?;
        self.relist(thread);
        if !delivered {
            return Err(Error::unavailable(
                "the harness no longer waits for this request",
            ));
        }
        Ok(request)
    }

    async fn withdraw_one(
        &self,
        thread: ThreadId,
        id: RequestId,
        reason: Withdrawal,
    ) -> Result<(), Error> {
        let changed = self
            .db
            .write(move |conn| {
                let Some(mut request) = store::request(conn, thread, id)? else {
                    return Ok(false);
                };
                let asker = Asker { parent: None };
                let Ok(state) = requests::decide(
                    &request.state,
                    &request.body,
                    asker,
                    Command::Withdraw(reason),
                    store::now(),
                ) else {
                    return Ok(false);
                };
                request.state = state;
                store::save_request(conn, &request)?;
                Ok(true)
            })
            .await?;
        if changed {
            self.relist(thread);
        }
        Ok(())
    }

    /// Chooses the thread's access mode, switching the harness's own mode
    /// when the chosen one has one.
    pub(crate) async fn set_thread_access(
        self: &Arc<Self>,
        thread: ThreadId,
        mode: Option<AccessMode>,
    ) -> Result<(), Error> {
        let (mode, target) = self
            .db
            .read_snapshot(move |tx| {
                let options = store::binding(tx, thread)?.session.options;
                let mode = mode
                    .map(|mode| requests::choose(&options, &mode))
                    .transpose()?;
                let (_, project) = store::chosen_access(tx, thread)?;
                let target = mode.clone().or(project);
                Ok((mode, target))
            })
            .await?;
        if let Some(native) = target.and_then(|target| target.native) {
            self.configure(thread, native.option, OptionValue::Select(native.value))
                .await?;
        }
        self.db
            .write(move |conn| store::save_thread_access(conn, thread, mode.as_ref()))
            .await?;
        self.relist(thread);
        Ok(())
    }

    /// Sets a project's default access mode for one harness. A harness mode
    /// must be one the harness has reported.
    pub(crate) async fn set_project_access(&self, input: SetProjectAccess) -> Result<(), Error> {
        let SetProjectAccess {
            project,
            harness,
            access,
        } = input;
        let (_, config) = self.harness(Some(&harness))?;
        let config = config.clone();
        let project_for_emit = project.clone();
        self.db
            .write_tx(move |tx| {
                store::require_project(tx, &project)?;
                let access = match access {
                    Some(mode) => {
                        let options = sessions::catalog(tx, &harness)?
                            .map_or_else(Vec::new, |catalog| catalog.options(&config, None));
                        Some(requests::choose(&options, &mode)?)
                    }
                    None => None,
                };
                store::save_project_access(tx, &project, &harness, access.as_ref())
            })
            .await?;
        self.project_changed(project_for_emit);
        Ok(())
    }
}
