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
    sql::{self, Conn, exec, row},
    store::{self, Handoff, NewThread, SavedSession},
    worktree::{self, Placement},
};
use crate::error::Error;

impl Workspace {
    /// A worktree is created before the fork is recorded, and removed again if
    /// the fork fails or repeats an earlier request, in a tracked job that
    /// outlives a caller who stops waiting.
    pub(super) async fn fork(self: &Arc<Self>, args: ForkThread) -> Result<Forked, Error> {
        threads::validate_request_id(args.request_id.as_deref())?;
        threads::validate_title(args.title.as_deref())?;
        let Some(target) = args.worktree.clone() else {
            return self.fork_in(args, None).await.map(|(forked, _)| forked);
        };
        let source_thread = args.thread;
        let request_id = args.request_id.clone();
        let precheck = self
            .db
            .read_snapshot(move |tx| {
                if let Some(found) = by_request(tx, request_id.as_deref())? {
                    return Ok(Err(found));
                }
                let binding = store::binding(tx, source_thread)?;
                let (metadata, _) = store::metadata(tx, source_thread)?;
                Ok(Ok((binding, metadata.title().to_owned())))
            })
            .await?;
        let (binding, title) = match precheck {
            Ok(pair) => pair,
            Err(forked) => return Ok(forked),
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
        let workspace = Arc::clone(self);
        self.jobs
            .spawn(async move {
                let placement = workspace
                    .place(&project, Path::new(&source), &target, &title)
                    .await?;
                let result = workspace.fork_in(args, Some(&placement)).await;
                if !matches!(result, Ok((_, true))) {
                    worktree::discard(&workspace.worktrees, Path::new(&project), &placement).await;
                }
                result.map(|(forked, _)| forked)
            })
            .await
            .map_err(viz_git::Error::from)?
    }

    /// Record the fork, or return the one an earlier request with the same ID
    /// made (with `false`).
    async fn fork_in(
        self: &Arc<Self>,
        args: ForkThread,
        placement: Option<&Placement>,
    ) -> Result<(Forked, bool), Error> {
        let scratch = if placement.is_none() {
            let source_thread = args.thread;
            let request_id = args.request_id.clone();
            let custom_title = args.title.clone();
            let precheck = self
                .db
                .read_snapshot(move |tx| {
                    if let Some(found) = by_request(tx, request_id.as_deref())? {
                        return Ok(Err(found));
                    }
                    let binding = store::binding(tx, source_thread)?;
                    let title = if binding.project.is_none() {
                        let (metadata, _) = store::metadata(tx, source_thread)?;
                        Some(custom_title.unwrap_or_else(|| metadata.title().to_owned()))
                    } else {
                        None
                    };
                    Ok(Ok(title))
                })
                .await?;
            match precheck {
                Err(forked) => return Ok((forked, false)),
                Ok(Some(title)) => Some(self.prepare_scratch(&title).await?),
                Ok(None) => None,
            }
        } else {
            None
        };
        let result = self.fork_tx(args, placement, scratch.as_ref()).await;
        if !matches!(result, Ok((_, true)))
            && let Some(scratch) = scratch
        {
            scratch.discard().await;
        }
        result
    }

    #[expect(
        clippy::too_many_lines,
        reason = "single atomic SQLite transaction recording a forked thread, copied history, and idempotency receipt"
    )]
    async fn fork_tx(
        self: &Arc<Self>,
        args: ForkThread,
        placement: Option<&Placement>,
        scratch: Option<&super::scratch::PreparedScratch>,
    ) -> Result<(Forked, bool), Error> {
        let source = args.thread;
        let placement_checkout = placement.map(|p| p.checkout.clone());
        let scratch_cwd = scratch.map(|s| s.cwd.clone());
        let ws = Arc::clone(self);
        let (forked, created) = self
            .db
            .write(move |conn| {
                let tx = sql::immediate(conn)?;
                if let Some(found) = by_request(&tx, args.request_id.as_deref())? {
                    return Ok((found, false));
                }
                let binding = store::binding(&tx, source)?;
                let (metadata, _) = store::metadata(&tx, source)?;
                let prompt = match args.point {
                    ForkPoint::Before { entry } => Some(
                        store::fork_prompt(&tx, source, entry)?
                            .ok_or(fork::Rejection::NotAPrompt(entry))?,
                    ),
                    ForkPoint::Head | ForkPoint::Through { .. } => None,
                };
                let (turns, last) = store::turns(&tx, source)?;
                let target_exists = match args.point {
                    ForkPoint::Through { entry } => store::has_entry(&tx, source, entry)?,
                    ForkPoint::Head | ForkPoint::Before { .. } => true,
                };
                let cut = fork::cut(&turns, last, args.point, target_exists)?;
                let (harness, _) =
                    ws.harness(Some(args.harness.as_deref().unwrap_or(&binding.harness)))?;
                let same_harness = harness == binding.harness;
                let scratch_checkout = (binding.project.is_none() && placement_checkout.is_none())
                    .then(|| {
                        scratch_cwd.map(|cwd| viz_core::placement::Checkout {
                            cwd,
                            worktree: None,
                        })
                    })
                    .flatten();
                if binding.project.is_none()
                    && placement_checkout.is_none()
                    && scratch_checkout.is_none()
                {
                    return Err(Error::unavailable(
                        "source thread project changed during fork; retry",
                    ));
                }
                let has_placement = placement_checkout.is_some();
                let has_scratch = scratch_checkout.is_some();
                let checkout = match (placement_checkout, scratch_checkout) {
                    (Some(checkout), _) | (None, Some(checkout)) => checkout,
                    (None, None) => binding.checkout,
                };
                let native = binding
                    .session
                    .session_id
                    .clone()
                    .filter(|_| same_harness && !has_placement && !has_scratch);
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
                    &ws.reservations,
                    &tx,
                    &NewThread {
                        harness,
                        checkout: &checkout,
                        project: binding.project.as_deref(),
                        parent: None,
                        parent_environment: None,
                        title: args.title.as_deref(),
                        session,
                    },
                )?;
                let shared = store::copy_history(&tx, source, thread, &cut)?;
                if let Some(prompt) = &prompt {
                    copy_prompt_attachments(&tx, source, thread, prompt.attachments())?;
                }
                exec(
                    &tx,
                    "UPDATE threads SET forked_from = ?2, forked_at = ?3 WHERE id = ?1",
                    [thread.into(), source.into(), shared.into()],
                )?;
                if args.title.is_none() {
                    let title = Title::Derived(metadata.title().to_owned());
                    let metadata = threads::Metadata { title, ..metadata };
                    store::save_metadata(&tx, thread, &metadata)?;
                }
                if let Some(request_id) = args.request_id {
                    exec(
                        &tx,
                        "INSERT INTO fork_requests (request_id, thread, prompt) VALUES (?1, ?2, ?3)",
                        [request_id.into(), thread.into(), prompt.as_ref().map(store::json).transpose()?.into()],
                    )?;
                }
                tx.commit()?;
                ws.kick(thread, true);
                Ok((Forked { thread, prompt }, true))
            })
            .await?;
        if created {
            self.relist(forked.thread);
        }
        Ok((forked, created))
    }
}

fn by_request(db: &Conn, request_id: Option<&str>) -> Result<Option<Forked>, Error> {
    let Some(request_id) = request_id else {
        return Ok(None);
    };
    row(
        db,
        "SELECT thread, prompt FROM fork_requests WHERE request_id = ?1",
        [request_id.into()],
    )?
    .map(|mut receipt| {
        Ok(Forked {
            thread: receipt.id(0)?,
            prompt: receipt
                .opt_str(1)?
                .as_deref()
                .map(store::from_json)
                .transpose()?,
        })
    })
    .transpose()
}

fn copy_prompt_attachments(
    tx: &Conn,
    source: viz_protocol::ThreadId,
    target: viz_protocol::ThreadId,
    attachments: impl Iterator<Item = viz_protocol::PromptAttachment>,
) -> Result<(), Error> {
    for attachment in attachments {
        exec(
            tx,
            "INSERT INTO thread_attachments (thread, id, blob, name, mime)
             SELECT ?1, id, blob, name, mime
             FROM thread_attachments WHERE thread = ?2 AND id = ?3
             ON CONFLICT DO NOTHING",
            [target.into(), source.into(), attachment.id.as_str().into()],
        )?;
    }
    Ok(())
}
