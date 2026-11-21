//! Restore a Git snapshot and continue from a bounded transcript in a fresh ACP
//! session, or fork the harness session where the boundary allows.

use std::{
    collections::HashMap,
    os::unix::ffi::OsStrExt as _,
    path::{Path, PathBuf},
};

use git2::{
    Delta, DiffOptions, FileMode, Repository, Status, StatusOptions, Tree, build::CheckoutBuilder,
};
use toasty::Executor;
use viz_client::protocol::{ForkThread, RunBoundary, WorktreeRequest};

use super::{
    Workspace,
    checkpoints::{load_tree, working_tree},
    git,
    sql::{exec, row, rows},
    store::{self, NewThread, SavedSession},
};
use crate::wire::{Entry, EntryId, NewEntry, RpcError, ThreadId};

struct Boundary {
    entry: u64,
    checkpoint: Option<String>,
}

impl Workspace {
    pub(super) async fn seed_prompt<'a>(
        &self,
        thread: u64,
        text: std::borrow::Cow<'a, str>,
    ) -> Result<(bool, std::borrow::Cow<'a, str>), RpcError> {
        let seed = read_seed(&mut *self.db.lock().await, thread).await?;
        if seed.is_empty() {
            return Ok((false, text));
        }
        Ok((
            true,
            format!(
                "Earlier conversation for context. Continue only with the new user input below.

{seed}

New user input:
{text}"
            )
            .into(),
        ))
    }

    pub(super) async fn fork(&self, args: ForkThread) -> Result<ThreadId, RpcError> {
        let mut db = self.db.lock().await;
        let mut tx = db.transaction().await?;
        let source = args.thread.0;
        let binding = store::binding(&mut tx, source).await?;
        let boundary = boundary(&mut tx, source, args.run.0, args.boundary).await?;
        let fork_of = match &binding.session.session_id {
            Some(id) if is_latest(&mut tx, source, boundary.entry).await? => Some(id.clone()),
            _ => None,
        };
        let title = match args.title {
            Some(title) if title.trim().is_empty() => {
                return Err(RpcError::invalid("thread title must not be empty"));
            }
            Some(title) => title,
            None => {
                let Some(mut title) = row(
                    &mut tx,
                    "SELECT title FROM threads WHERE id = ?1",
                    [source.into()],
                )
                .await?
                else {
                    return Err(store::missing(source));
                };
                format!("{} (fork)", title.str(0)?)
            }
        };
        let pending = if let Some(worktree) = args.worktree {
            let checkpoint = boundary
                .checkpoint
                .ok_or_else(|| RpcError::not_found("no Git checkpoint at the fork boundary"))?;
            let cwd = binding.cwd.clone();
            let base = tokio::task::spawn_blocking(move || {
                let repo = git::open(Path::new(&cwd))?;
                let commit = repo.find_reference(&checkpoint)?.peel_to_commit()?;
                Ok::<_, RpcError>(commit.id().to_string())
            })
            .await
            .map_err(RpcError::internal)??;
            Some(
                self.provision(
                    &mut tx,
                    Path::new(&binding.cwd),
                    WorktreeRequest {
                        base_ref: Some(base),
                        branch: worktree.branch,
                    },
                    title.clone(),
                )
                .await?,
            )
        } else {
            None
        };
        let new = store::create_thread(
            &mut tx,
            &NewThread {
                harness: &binding.harness,
                cwd: pending.as_ref().map_or(binding.cwd.as_str(), |pending| {
                    pending.checkout.path.as_str()
                }),
                project: binding.project.as_deref(),
                parent: None,
                parent_environment: None,
                title: Some(&title),
                worktree_branch: pending
                    .as_ref()
                    .map(|pending| pending.checkout.branch.as_str()),
                session: SavedSession {
                    fork_of,
                    ..fresh_session(binding.session)
                },
            },
        )
        .await?;
        let entries = transcript(&mut tx, source, boundary.entry).await?;
        let seed = history_seed(entries.iter().rev());
        copy_transcript(&mut tx, source, new, entries).await?;
        set_history_seed(&mut tx, new, seed).await?;
        tx.commit().await?;
        if let Some(pending) = pending {
            pending.keep();
        }
        drop(db);
        self.relist(new);
        Ok(ThreadId(new))
    }

    pub(super) async fn restore(
        &self,
        thread: u64,
        run: u64,
        rewind: bool,
    ) -> Result<(), RpcError> {
        let driver = self.driver(thread);
        let mut slot = driver.session.lock().await;
        let mut db = self.db.lock().await;
        let mut tx = db.transaction().await?;
        let binding = store::binding(&mut tx, thread).await?;
        let boundary = boundary(&mut tx, thread, run, RunBoundary::Start).await?;
        let checkpoints = rows(
            &mut tx,
            "SELECT checkpoint_start, checkpoint_end FROM runs
            WHERE thread = ?1 AND hidden = 0 AND prompt_entry >= ?2 AND end_entry IS NOT NULL
            ORDER BY end_entry",
            [thread.into(), boundary.entry.into()],
        )
        .await?
        .map(|mut record| {
            let (Some(start), Some(end)) = (record.opt_str(0)?, record.opt_str(1)?) else {
                return Err(RpcError::not_found(
                    "a run in the restore range has no complete Git checkpoint",
                ));
            };
            Ok((start, end))
        })
        .collect::<Result<Vec<_>, _>>()?;
        let neighbors = rows(
            &mut tx,
            "SELECT DISTINCT t.cwd FROM threads t JOIN runs r ON r.thread = t.id
            WHERE r.status IN ('running', 'queued') OR (t.id <> ?1 AND r.end_entry >= ?2)",
            [thread.into(), boundary.entry.into()],
        )
        .await?
        .map(|mut row| row.str(0))
        .collect::<Result<Vec<_>, _>>()?;
        let seed = if rewind {
            history_seed(
                transcript(&mut tx, thread, boundary.entry)
                    .await?
                    .iter()
                    .rev(),
            )
        } else {
            String::new()
        };
        if rewind {
            let session = store::json(&fresh_session(binding.session))?;
            exec(
                &mut tx,
                "UPDATE entries SET hidden = 1 WHERE thread = ?1 AND id >= ?2",
                [thread.into(), boundary.entry.into()],
            )
            .await?;
            exec(
                &mut tx,
                "UPDATE runs SET hidden = 1 WHERE thread = ?1 AND prompt_entry >= ?2",
                [thread.into(), boundary.entry.into()],
            )
            .await?;
            exec(&mut tx, "UPDATE threads SET session = ?2, history_seed = ?3,
                history_revision = history_revision + 1, setup_failed = 0, updated_at = ?4 WHERE id = ?1",
                [thread.into(), session.into(), seed.into(), store::now().into()]).await?;
        }
        let cwd = PathBuf::from(&binding.cwd);
        tokio::task::spawn_blocking(move || restore_files(&cwd, &checkpoints, &neighbors))
            .await
            .map_err(RpcError::internal)??;
        tx.commit().await?;
        if rewind {
            slot.take();
        }
        drop(db);
        drop(slot);
        self.relist(thread);
        Ok(())
    }
}

async fn copy_transcript(
    db: &mut dyn Executor,
    source: u64,
    new: u64,
    entries: Vec<Entry>,
) -> Result<(), RpcError> {
    let mut tools = HashMap::new();
    for Entry { id, mut data } in entries {
        match &mut data {
            NewEntry::ToolUpdate { entry, .. } => {
                let Some(forked) = tools.get(entry) else {
                    continue;
                };
                *entry = *forked;
            }
            NewEntry::Run(end) => end.run = None,
            NewEntry::Prompt { attachments, .. } => {
                for attachment in attachments {
                    exec(
                        db,
                        "INSERT INTO attachments (thread, id, name, mime, data)
                        SELECT ?2, id, name, mime, data FROM attachments WHERE thread = ?1 AND id = ?3
                        ON CONFLICT DO NOTHING",
                        [source.into(), new.into(), attachment.id.as_str().into()],
                    ).await?;
                }
            }
            _ => {}
        }
        let copied = store::append(db, new, &data).await?;
        if matches!(data, NewEntry::Tool(_)) {
            tools.insert(id, EntryId(copied));
        }
    }
    Ok(())
}

fn fresh_session(session: SavedSession) -> SavedSession {
    SavedSession {
        desired: session.desired,
        selection: session.selection,
        ..SavedSession::default()
    }
}

async fn set_history_seed(
    db: &mut dyn Executor,
    thread: u64,
    seed: String,
) -> Result<(), RpcError> {
    exec(
        db,
        "UPDATE threads SET history_seed = ?2 WHERE id = ?1",
        [thread.into(), seed.into()],
    )
    .await?;
    Ok(())
}

async fn read_seed(db: &mut dyn Executor, thread: u64) -> Result<String, RpcError> {
    let mut record = row(
        db,
        "SELECT history_seed FROM threads WHERE id = ?1",
        [thread.into()],
    )
    .await?
    .ok_or_else(|| store::missing(thread))?;
    record.str(0)
}

pub(super) async fn clear_seed(db: &mut dyn Executor, thread: u64) -> Result<(), RpcError> {
    set_history_seed(db, thread, String::new()).await
}

async fn boundary(
    db: &mut dyn Executor,
    thread: u64,
    run: u64,
    boundary: RunBoundary,
) -> Result<Boundary, RpcError> {
    let mut record = row(
        db,
        "SELECT prompt_entry, end_entry, checkpoint_start, checkpoint_end
        FROM runs WHERE thread = ?1 AND id = ?2 AND hidden = 0 AND end_entry IS NOT NULL",
        [thread.into(), run.into()],
    )
    .await?
    .ok_or_else(|| RpcError::not_found("no settled run at this boundary"))?;
    let (entry, checkpoint) = match boundary {
        RunBoundary::Start => (record.u64(0)?, record.opt_str(2)?),
        RunBoundary::End => (record.u64(1)? + 1, record.opt_str(3)?),
    };
    Ok(Boundary { entry, checkpoint })
}

/// Whether nothing on `thread` follows `entry` and no run is pending, so the
/// harness session holds exactly the conversation before the boundary and
/// `session/fork` can copy it; the history seed covers every other boundary.
async fn is_latest(db: &mut dyn Executor, thread: u64, entry: u64) -> Result<bool, RpcError> {
    let record = row(
        db,
        "SELECT NOT EXISTS (SELECT 1 FROM entries WHERE thread = ?1 AND hidden = 0 AND id >= ?2)
            AND NOT EXISTS (SELECT 1 FROM runs WHERE thread = ?1 AND status IN ('running', 'queued'))",
        [thread.into(), entry.into()],
    )
    .await?
    .ok_or_else(|| store::missing(thread))?;
    Ok(record.u64(0)? == 1)
}

async fn transcript(
    db: &mut dyn Executor,
    thread: u64,
    before: u64,
) -> Result<Vec<Entry>, RpcError> {
    rows(
        db,
        "SELECT id, data FROM entries WHERE thread = ?1 AND id < ?2 AND hidden = 0 ORDER BY id",
        [thread.into(), before.into()],
    )
    .await?
    .map(|mut row| {
        Ok(Entry {
            id: EntryId(row.u64(0)?),
            data: store::from_json(&row.str(1)?)?,
        })
    })
    .collect()
}

/// Like t3code's bounded fork transcript, carry forward only context before
/// the selected boundary. ACP runners without native rewind receive it with
/// the first new prompt so historical prompts are never executed again.
fn history_seed<'a>(entries: impl Iterator<Item = &'a Entry>) -> String {
    let mut parts = Vec::new();
    let mut remaining = 24_000;
    for entry in entries {
        let (role, text) = match &entry.data {
            NewEntry::Prompt { text, .. } => ("User", text),
            NewEntry::Text { text, .. } => ("Assistant", text),
            NewEntry::TaskResult { text, .. } => ("Task result", text),
            _ => continue,
        };
        if text.trim().is_empty() {
            continue;
        }
        let mut len = remaining.min(text.len());
        while !text.is_char_boundary(text.len() - len) {
            len -= 1;
        }
        parts.push(format!("{role}: {}", &text[text.len() - len..]));
        remaining = remaining.saturating_sub(len + role.len() + 4);
        if remaining < 4 {
            break;
        }
    }
    parts.reverse();
    parts.join("\n\n")
}

fn restore_files(
    cwd: &Path,
    checkpoints: &[(String, String)],
    neighbors: &[String],
) -> Result<(), RpcError> {
    let repo = git::open(cwd)?;
    let root = repo.workdir().expect("checked workdir");
    if neighbors
        .iter()
        .any(|cwd| Repository::discover(cwd).is_ok_and(|other| other.workdir() == Some(root)))
    {
        return Err(RpcError::invalid(
            "another run is active in this checkout or changed it after the selected boundary",
        ));
    }
    let statuses = repo.statuses(Some(StatusOptions::new().include_ignored(true)))?;
    let staged = Status::INDEX_NEW
        | Status::INDEX_MODIFIED
        | Status::INDEX_DELETED
        | Status::INDEX_RENAMED
        | Status::INDEX_TYPECHANGE
        | Status::CONFLICTED;
    if statuses.iter().any(|item| item.status().intersects(staged)) {
        return Err(RpcError::invalid(
            "the checkout contains staged changes or conflicts",
        ));
    }
    let (reference, last) = checked_range(&repo, checkpoints)?;
    let previous = repo.find_reference(last)?.peel_to_commit()?;
    let head = repo.head().ok().and_then(|head| head.target());
    if head != previous.parent_ids().next() {
        return Err(RpcError::invalid(
            "the checkout HEAD changed since the latest run checkpoint",
        ));
    }
    let target = load_tree(&repo, reference)?;
    let current = working_tree(&repo)?;
    if current.id() == target.id() {
        return Ok(());
    }
    if current.id() != previous.tree_id() {
        return Err(RpcError::invalid(
            "the checkout changed since the latest run checkpoint",
        ));
    }
    let baseline = previous.tree()?;
    let (remove, write) = changes(&repo, &baseline, &target, &statuses)?;
    let checkout = git::open(cwd)?;
    if let Err(error) = apply_tree(&checkout, target.id(), &remove, &write) {
        let rollback = rollback_partial(cwd, &baseline, &statuses, &remove, &write);
        if let Err(rollback) = rollback {
            return Err(RpcError::unavailable(format!(
                "restoring checkpoint failed: {error}; restoring the previous working tree also failed: {rollback}"
            )));
        }
        return Err(error);
    }
    Ok(())
}

/// A gap between one run's end and the next run's start belongs to the
/// checkout outside either run. Refuse to cross it when restoring.
fn checked_range<'a>(
    repo: &Repository,
    checkpoints: &'a [(String, String)],
) -> Result<(&'a str, &'a str), RpcError> {
    let Some((first, _)) = checkpoints.first() else {
        return Err(RpcError::not_found(
            "no Git checkpoints in the restore range",
        ));
    };
    for pair in checkpoints.windows(2) {
        let previous = repo.find_reference(&pair[0].1)?.peel_to_commit()?;
        let next = repo.find_reference(&pair[1].0)?.peel_to_commit()?;
        if previous.tree_id() != next.tree_id()
            || previous.parent_ids().next() != next.parent_ids().next()
        {
            return Err(RpcError::invalid(
                "the checkout changed between runs in the restore range",
            ));
        }
    }
    let (_, last) = checkpoints
        .last()
        .expect("checked nonempty checkpoint range");
    Ok((first, last))
}

fn changes(
    repo: &Repository,
    baseline: &Tree<'_>,
    target: &Tree<'_>,
    statuses: &git2::Statuses<'_>,
) -> Result<(Vec<PathBuf>, Vec<PathBuf>), RpcError> {
    let mut options = DiffOptions::new();
    options.include_typechange(true);
    let diff = repo.diff_tree_to_tree(Some(baseline), Some(target), Some(&mut options))?;
    let mut remove = Vec::new();
    let mut write = Vec::new();
    for delta in diff.deltas() {
        let old = delta.old_file();
        let new = delta.new_file();
        if old.mode() == FileMode::Commit || new.mode() == FileMode::Commit {
            return Err(RpcError::invalid(
                "restoring changes to submodules is unsupported",
            ));
        }
        let path = new
            .path()
            .or_else(|| old.path())
            .ok_or_else(|| RpcError::internal("Git delta lacks a path"))?;
        if statuses
            .iter()
            .filter(|item| item.status().contains(Status::IGNORED))
            .any(|item| {
                let ignored = Path::new(std::ffi::OsStr::from_bytes(item.path_bytes()));
                ignored.starts_with(path) || path.starts_with(ignored)
            })
        {
            return Err(RpcError::invalid(
                "restoring this checkpoint would affect ignored files",
            ));
        }
        if delta.status() == Delta::Deleted {
            remove.push(path.to_owned());
        } else {
            write.push(path.to_owned());
        }
    }
    Ok((remove, write))
}

fn rollback_partial(
    cwd: &Path,
    baseline: &Tree<'_>,
    statuses: &git2::Statuses<'_>,
    attempted_remove: &[PathBuf],
    attempted_write: &[PathBuf],
) -> Result<(), RpcError> {
    let repo = git::open(cwd)?;
    let current = working_tree(&repo)?;
    let baseline = repo.find_tree(baseline.id())?;
    let (remove, write) = changes(&repo, &current, &baseline, statuses)?;
    if remove
        .iter()
        .chain(&write)
        .any(|path| !attempted_remove.contains(path) && !attempted_write.contains(path))
    {
        return Err(RpcError::invalid(
            "unrelated paths changed while restoring the checkpoint",
        ));
    }
    let checkout = git::open(cwd)?;
    apply_tree(&checkout, baseline.id(), &remove, &write)
}

fn apply_tree(
    checkout: &Repository,
    target: git2::Oid,
    remove: &[PathBuf],
    write: &[PathBuf],
) -> Result<(), RpcError> {
    let tree = checkout.find_tree(target)?;
    let root = checkout.workdir().expect("checked workdir");
    for path in remove {
        std::fs::remove_file(root.join(path)).map_err(RpcError::internal)?;
    }
    if !write.is_empty() {
        let mut options = CheckoutBuilder::new();
        options
            .force()
            .disable_pathspec_match(true)
            .update_index(false)
            .overwrite_ignored(false);
        for path in write {
            let absolute = root.join(path);
            if absolute.is_dir() && !absolute.is_symlink() {
                std::fs::remove_dir(&absolute).map_err(RpcError::internal)?;
            }
            options.path(path);
        }
        checkout.checkout_tree(tree.as_object(), Some(&mut options))?;
    }
    if working_tree(checkout)?.id() != target {
        return Err(RpcError::unavailable(
            "checkout differs from the requested snapshot after restoration",
        ));
    }
    Ok(())
}
