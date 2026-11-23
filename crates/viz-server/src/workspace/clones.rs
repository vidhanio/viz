//! Async project provisioning using the Git edge's cancellable clone.

use std::{collections::HashMap, path::Path, sync::Arc, time::Instant};

use tokio::sync::{Mutex, watch};
use tokio_util::sync::CancellationToken;
use viz_protocol::{CloneId, ClonePhase, CloneState, CloneStatus, StartClone};

use super::{Workspace, folders::expand_path, store};
use crate::error::Error;

#[derive(Debug, Default)]
pub(super) struct Clones(Mutex<HashMap<CloneId, Operation>>);

#[derive(Debug)]
struct Operation {
    cancel: CancellationToken,
    state: watch::Receiver<CloneState>,
    started: Instant,
}

impl Workspace {
    pub(super) async fn start_clone(
        self: &Arc<Self>,
        args: StartClone,
    ) -> Result<CloneStatus, Error> {
        if args.path.trim().is_empty()
            || args.url.trim().is_empty()
            || args
                .name
                .as_deref()
                .is_some_and(|name| name.trim().is_empty())
        {
            return Err(Error::invalid(
                "clone source, destination and name must not be empty",
            ));
        }
        let path = expand_path(self.default_cwd(), Path::new(&args.path));
        let name = path
            .file_name()
            .ok_or_else(|| Error::invalid("clone destination needs a directory name"))?;
        let parent = path
            .parent()
            .ok_or_else(|| Error::invalid("clone destination needs a parent"))?;
        let destination = store::directory(parent).await?.join(name);
        if tokio::fs::symlink_metadata(&destination).await.is_ok() {
            return Err(Error::invalid("clone destination already exists"));
        }
        let project = store::path_text(&destination)?.to_owned();
        let id = CloneId(uuid::Uuid::new_v4().to_string());
        let initial = CloneState::Running {
            phase: ClonePhase::Preparing,
            percent: None,
        };
        let (send, state) = watch::channel(initial.clone());
        let cancel = self.closing.child_token();
        let mut operations = self.clones.0.lock().await;
        if operations.len() >= 64 {
            let oldest = operations
                .iter()
                .filter(|(_, operation)| !running(&operation.state.borrow()))
                .min_by_key(|(_, operation)| operation.started)
                .map(|(id, _)| id.clone())
                .ok_or_else(|| Error::unavailable("too many clone operations"))?;
            operations.remove(&oldest);
        }
        operations.insert(
            id.clone(),
            Operation {
                cancel: cancel.clone(),
                state,
                started: Instant::now(),
            },
        );
        drop(operations);
        let workspace = Arc::clone(self);
        self.jobs.spawn(async move {
            let result = viz_git::clone_repository(&args.url, &destination, &cancel, |progress| {
                send.send_replace(CloneState::Running {
                    phase: phase(progress.phase),
                    percent: progress.percent,
                });
            })
            .await;
            let state = match result {
                Ok(()) => match workspace.create_project(&project, args.name).await {
                    Ok(_) => CloneState::Completed { project },
                    Err(error) => CloneState::Failed {
                        message: error.to_string(),
                    },
                },
                Err(viz_git::Error::Cancelled) => CloneState::Cancelled,
                Err(error) => CloneState::Failed {
                    message: error.to_string(),
                },
            };
            send.send_replace(state);
        });
        Ok(CloneStatus { id, state: initial })
    }

    pub(super) async fn clone_status(&self, id: CloneId) -> Result<CloneStatus, Error> {
        self.clones.status(id, CloneRequest::Read).await
    }

    pub(super) async fn cancel_clone(&self, id: CloneId) -> Result<CloneStatus, Error> {
        self.clones.status(id, CloneRequest::Cancel).await
    }
}

#[derive(Clone, Copy)]
enum CloneRequest {
    Read,
    Cancel,
}

impl Clones {
    async fn status(&self, id: CloneId, request: CloneRequest) -> Result<CloneStatus, Error> {
        let operations = self.0.lock().await;
        let operation = operations
            .get(&id)
            .ok_or_else(|| Error::not_found("unknown clone operation"))?;
        if matches!(request, CloneRequest::Cancel) {
            operation.cancel.cancel();
        }
        let state = operation.state.borrow().clone();
        drop(operations);
        Ok(CloneStatus { id, state })
    }
}

const fn running(state: &CloneState) -> bool {
    matches!(state, CloneState::Running { .. })
}

const fn phase(phase: viz_git::ClonePhase) -> ClonePhase {
    match phase {
        viz_git::ClonePhase::Counting => ClonePhase::Counting,
        viz_git::ClonePhase::Compressing => ClonePhase::Compressing,
        viz_git::ClonePhase::Receiving => ClonePhase::Receiving,
        viz_git::ClonePhase::Resolving => ClonePhase::Resolving,
        viz_git::ClonePhase::CheckingOut => ClonePhase::CheckingOut,
    }
}
