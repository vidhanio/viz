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
use viz_acp::{
    ClientEvents, ClientRequest, LocalIo, PermissionPolicy, PtyTerminals, TerminalAccess,
};
use viz_core::{
    lifecycle,
    requests::{self, Admission, Asker, Command},
};
use viz_protocol::{
    AccessMode, AnswerRequest, AnsweredBy, OptionValue, RequestAnswer, RequestBody, RequestId,
    RunId, RunRequest, SetProjectAccess, ThreadId, Withdrawal,
};

use super::{Workspace, sessions, store};
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
    /// this workspace, which applies the thread's access mode, and commands
    /// run in PTYs clients can attach to.
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
                terminals: TerminalAccess::Pty(PtyTerminals::new(
                    Arc::clone(&self.terminals),
                    thread,
                )),
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
        let admitted = match self.admit(thread, &body).await {
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
    async fn admit(&self, thread: ThreadId, body: &RequestBody) -> Result<Admitted, Error> {
        let Some(run) = self
            .workers
            .get(thread)
            .and_then(|driver| driver.active_run())
        else {
            return Ok(Admitted::Refuse);
        };
        let mut db = self.db.lock().await;
        let options = store::binding(&mut db, thread).await?.session.options;
        let access = store::access(&mut db, thread, &options).await?;
        if let Admission::Answer(answer) = requests::admit(access.current.kind, body) {
            return Ok(Admitted::Answer(answer));
        }
        let watched = self.is_watched(thread);
        let mut tx = db.transaction().await?;
        let request = store::insert_request(&mut tx, thread, run, body).await?;
        store::touch(&mut tx, thread, |current| {
            lifecycle::on_request(current, watched)
        })
        .await?;
        tx.commit().await?;
        let (sender, receiver) = oneshot::channel();
        self.requests.waiters().insert(
            request.id,
            Waiter {
                run,
                answer: sender,
            },
        );
        drop(db);
        self.relist(thread);
        Ok(Admitted::Wait(request.id, receiver))
    }

    /// Resolves a pending request for the user or the delegating agent.
    pub(crate) async fn answer(
        &self,
        input: AnswerRequest,
        by: AnsweredBy,
    ) -> Result<RunRequest, Error> {
        let AnswerRequest {
            thread,
            request: id,
            answer,
        } = input;
        let mut db = self.db.lock().await;
        let mut request = store::request(&mut db, thread, id)
            .await?
            .ok_or_else(|| Error::not_found(format!("no request {id} in thread {thread}")))?;
        let binding = store::binding(&mut db, thread).await?;
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
        let delivered = self
            .requests
            .waiters()
            .remove(&id)
            .is_some_and(|waiter| waiter.answer.send(answer).is_ok());
        if !delivered {
            drop(db);
            self.withdraw_one(thread, id, Withdrawal::Harness).await?;
            return Err(Error::unavailable(
                "the harness no longer waits for this request",
            ));
        }
        store::save_request(&mut db, &request).await?;
        drop(db);
        self.relist(thread);
        Ok(request)
    }

    async fn withdraw_one(
        &self,
        thread: ThreadId,
        id: RequestId,
        reason: Withdrawal,
    ) -> Result<(), Error> {
        let mut db = self.db.lock().await;
        let Some(mut request) = store::request(&mut db, thread, id).await? else {
            return Ok(());
        };
        let asker = Asker { parent: None };
        let Ok(state) = requests::decide(
            &request.state,
            &request.body,
            asker,
            Command::Withdraw(reason),
            store::now(),
        ) else {
            return Ok(());
        };
        request.state = state;
        store::save_request(&mut db, &request).await?;
        drop(db);
        self.relist(thread);
        Ok(())
    }

    /// Chooses the thread's access mode, switching the harness's own mode
    /// when the chosen one has one.
    pub(crate) async fn set_thread_access(
        self: &Arc<Self>,
        thread: ThreadId,
        mode: Option<AccessMode>,
    ) -> Result<(), Error> {
        let (mode, target) = {
            let mut db = self.db.lock().await;
            let options = store::binding(&mut db, thread).await?.session.options;
            let mode = mode
                .map(|mode| requests::choose(&options, &mode))
                .transpose()?;
            let (_, project) = store::chosen_access(&mut db, thread).await?;
            let target = mode.clone().or(project);
            drop(db);
            (mode, target)
        };
        if let Some(native) = target.and_then(|target| target.native) {
            self.configure(thread, native.option, OptionValue::Select(native.value))
                .await?;
        }
        store::save_thread_access(&mut *self.db.lock().await, thread, mode.as_ref()).await?;
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
        let mut db = self.db.lock().await;
        store::project(&mut db, &project).await?;
        let access = match access {
            Some(mode) => {
                let options = sessions::catalog(&mut db, &harness)
                    .await?
                    .map_or_else(Vec::new, |catalog| catalog.options(config, None));
                Some(requests::choose(&options, &mode)?)
            }
            None => None,
        };
        store::save_project_access(&mut db, &project, &harness, access.as_ref()).await?;
        drop(db);
        self.changed(None);
        Ok(())
    }
}
