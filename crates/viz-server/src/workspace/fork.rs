//! Fork a thread into a new one that shares part of its history.

use std::{path::Path, sync::Arc};

use viz_core::{
    fork::{self, Transfer},
    placement::Placed,
    threads::{self, Title},
};
use viz_protocol::{ForkPoint, ForkThread, Forked};

use super::{
    Workspace,
    sql::exec,
    store::{self, Handoff, NewThread, SavedSession},
    thread::launched,
    worktree::{self, Placement},
};
use crate::error::Error;

impl Workspace {
    /// A worktree is created before the fork is recorded, and removed again if
    /// the fork fails or repeats an earlier request.
    pub(super) async fn fork(self: &Arc<Self>, args: ForkThread) -> Result<Forked, Error> {
        threads::validate_request_id(args.request_id.as_deref())?;
        threads::validate_title(args.title.as_deref())?;
        let Some(target) = args.worktree.clone() else {
            return self.fork_in(args, None).await.map(|(forked, _)| forked);
        };
        let (binding, title) = {
            let mut db = self.db.lock().await;
            if let Some(found) = launched(&mut db, args.request_id.as_deref()).await? {
                return Ok(Forked {
                    thread: found.thread,
                    prompt: None,
                });
            }
            let binding = store::binding(&mut db, args.thread).await?;
            let (metadata, _) = store::metadata(&mut db, args.thread).await?;
            drop(db);
            (binding, metadata.title().to_owned())
        };
        let project = binding
            .project
            .clone()
            .ok_or(threads::Rejection::NoProject)?;
        let source = Placed {
            project: Some(&project),
            checkout: &binding.checkout,
            within: self.within(&binding).await,
        }
        .source(&project)
        .to_owned();
        let title = args.title.clone().unwrap_or(title);
        let placement = self
            .place(&project, Path::new(&source), &target, &title)
            .await?;
        let result = self.fork_in(args, Some(&placement)).await;
        if !matches!(result, Ok((_, true))) {
            worktree::discard(&self.worktrees, Path::new(&project), &placement).await;
        }
        result.map(|(forked, _)| forked)
    }

    /// Record the fork, or return the one an earlier request with the same ID
    /// made (with `false`).
    async fn fork_in(
        self: &Arc<Self>,
        args: ForkThread,
        placement: Option<&Placement>,
    ) -> Result<(Forked, bool), Error> {
        let source = args.thread;
        let mut db = self.db.lock().await;
        let mut tx = db.transaction().await?;
        if let Some(found) = launched(&mut tx, args.request_id.as_deref()).await? {
            let forked = Forked {
                thread: found.thread,
                prompt: None,
            };
            return Ok((forked, false));
        }
        let binding = store::binding(&mut tx, source).await?;
        let (metadata, _) = store::metadata(&mut tx, source).await?;
        let prompt = match args.point {
            ForkPoint::Before { entry } => Some(
                store::fork_prompt(&mut tx, source, entry)
                    .await?
                    .ok_or(fork::Rejection::NotAPrompt(entry))?,
            ),
            ForkPoint::Head | ForkPoint::Through { .. } => None,
        };
        let (turns, last) = store::turns(&mut tx, source).await?;
        let cut = fork::cut(&turns, last, args.point)?;
        let (harness, _) =
            self.harness(Some(args.harness.as_deref().unwrap_or(&binding.harness)))?;
        let same_harness = harness == binding.harness;
        let checkout = placement.map_or(&binding.checkout, |placement| &placement.checkout);
        let native = binding
            .session
            .session_id
            .clone()
            .filter(|_| same_harness && placement.is_none());
        let session = SavedSession {
            desired: if same_harness {
                binding.session.desired
            } else {
                SavedSession::default().desired
            },
            handoff: match fork::transfer(&cut, native.is_some()) {
                Transfer::Native => Some(Handoff::Forked { session: native }),
                Transfer::Handoff => Some(Handoff::Forked { session: None }),
                Transfer::Nothing => None,
            },
            ..SavedSession::default()
        };
        let thread = store::create_thread(
            self,
            &mut tx,
            &NewThread {
                harness,
                checkout,
                project: binding.project.as_deref(),
                parent: None,
                parent_environment: None,
                title: args.title.as_deref(),
                session,
            },
        )
        .await?;
        let shared = store::copy_history(&mut tx, source, thread, &cut).await?;
        if let Some(prompt) = &prompt {
            copy_prompt_attachments(&mut tx, source, thread, &prompt.attachments).await?;
        }
        exec(
            &mut tx,
            "UPDATE threads SET forked_from = ?2, forked_at = ?3 WHERE id = ?1",
            [thread.into(), source.into(), shared.into()],
        )
        .await?;
        let scratch_title = binding.project.is_none().then(|| {
            args.title
                .as_deref()
                .unwrap_or_else(|| metadata.title())
                .to_owned()
        });
        if args.title.is_none() {
            let title = Title::Derived(metadata.title().to_owned());
            let metadata = threads::Metadata { title, ..metadata };
            store::save_metadata(&mut tx, thread, &metadata).await?;
        }
        if let Some(request_id) = args.request_id {
            exec(
                &mut tx,
                "INSERT INTO launch_requests (request_id, thread, run) VALUES (?1, ?2, NULL)",
                [request_id.into(), thread.into()],
            )
            .await?;
        }
        self.commit_thread(tx, thread, scratch_title.as_deref())
            .await?;
        self.kick(thread, true);
        drop(db);
        self.relist(thread);
        Ok((Forked { thread, prompt }, true))
    }
}

async fn copy_prompt_attachments(
    tx: &mut super::sql::Connection,
    source: viz_protocol::ThreadId,
    target: viz_protocol::ThreadId,
    attachments: &[viz_protocol::PromptAttachment],
) -> Result<(), Error> {
    for attachment in attachments {
        exec(
            tx,
            "INSERT INTO attachments (thread, id, name, mime, data)
            SELECT ?1, id, name, mime, data FROM attachments WHERE thread = ?2 AND id = ?3
            ON CONFLICT DO NOTHING",
            [target.into(), source.into(), attachment.id.as_str().into()],
        )
        .await?;
    }
    Ok(())
}
