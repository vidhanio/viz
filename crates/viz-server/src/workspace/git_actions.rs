//! User-initiated Git writes against the checkout bound to a thread.

use std::path::Path;

use git2::{
    Cred, CredentialType, ErrorCode, IndexAddOption, PushOptions, RemoteCallbacks, Repository,
    RepositoryState,
};
use serde_json::Value;
use viz_client::protocol::{
    CommitGit, CreateGitBranch, GitBranch, GitCommit, GitPush, Method, PushGit,
};

use super::{Workspace, git, sql::rows, store};
use crate::wire::{RpcError, decode, encode};

impl Workspace {
    pub(super) async fn git_action(&self, method: &str, params: Value) -> Result<Value, RpcError> {
        match method {
            CommitGit::NAME => {
                let input: CommitGit = decode(params)?;
                let thread = input.thread.0;
                let mut db = self.db.lock().await;
                let cwd = store::binding(&mut *db, thread).await?.cwd;
                let running = rows(
                    &mut *db,
                    "SELECT DISTINCT t.cwd FROM threads t JOIN runs r ON r.thread = t.id
                    WHERE r.status IN ('running', 'queued')",
                    [],
                )
                .await?
                .map(|mut row| row.str(0))
                .collect::<Result<Vec<_>, _>>()?;
                let commit = blocking(move || {
                    let repo = git::open(Path::new(&cwd))?;
                    let root = repo.workdir();
                    if running.iter().any(|cwd| {
                        Repository::discover(cwd).is_ok_and(|other| other.workdir() == root)
                    }) {
                        return Err(RpcError::invalid(
                            "stop pending runs in this checkout before committing",
                        ));
                    }
                    commit(&repo, &input)
                })
                .await?;
                drop(db);
                self.relist(thread);
                encode(commit)
            }
            CreateGitBranch::NAME => {
                let input: CreateGitBranch = decode(params)?;
                let cwd = store::binding(&mut *self.db.lock().await, input.thread.0)
                    .await?
                    .cwd;
                let branch = blocking(move || {
                    let repo = git::open(Path::new(&cwd))?;
                    let base = repo
                        .revparse_single(input.base_ref.as_deref().unwrap_or("HEAD"))?
                        .peel_to_commit()?;
                    let branch = repo.branch(&input.branch, &base, false)?;
                    Ok(GitBranch {
                        reference: branch.get().name()?.to_owned(),
                        head: base.id().to_string(),
                    })
                })
                .await?;
                self.changed(None);
                encode(branch)
            }
            PushGit::NAME => {
                let input: PushGit = decode(params)?;
                let thread = input.thread.0;
                let cwd = store::binding(&mut *self.db.lock().await, thread)
                    .await?
                    .cwd;
                let pushed = blocking(move || {
                    let repo = git::open(Path::new(&cwd))?;
                    push(&repo, &input)
                })
                .await?;
                self.relist(thread);
                encode(pushed)
            }
            _ => unreachable!("not a Git action"),
        }
    }
}

async fn blocking<T: Send + 'static>(
    operation: impl FnOnce() -> Result<T, RpcError> + Send + 'static,
) -> Result<T, RpcError> {
    tokio::task::spawn_blocking(operation)
        .await
        .map_err(RpcError::internal)?
}

fn commit(repo: &Repository, input: &CommitGit) -> Result<GitCommit, RpcError> {
    if input.message.trim().is_empty() {
        return Err(RpcError::invalid("commit message must not be empty"));
    }
    if repo.state() != RepositoryState::Clean {
        return Err(RpcError::invalid(
            "finish the current Git operation before committing",
        ));
    }
    let signature = repo.signature()?;
    let parent = match repo.head() {
        Ok(head) => Some(head.peel_to_commit()?),
        Err(error) if error.code() == ErrorCode::UnbornBranch => None,
        Err(error) => return Err(error.into()),
    };
    let mut index = repo.index()?;
    if index.has_conflicts() {
        return Err(RpcError::invalid(
            "resolve index conflicts before committing",
        ));
    }
    if input.all {
        index.update_all(["*"], None)?;
        index.add_all(["*"], IndexAddOption::DEFAULT, None)?;
    }
    let tree = repo.find_tree(index.write_tree()?)?;
    if parent
        .as_ref()
        .is_some_and(|parent| parent.tree_id() == tree.id())
        || (parent.is_none() && tree.is_empty())
    {
        return Err(RpcError::invalid("no changes to commit"));
    }
    if input.all {
        index.write()?;
    }
    let parents: Vec<_> = parent.iter().collect();
    let head = repo.commit(
        Some("HEAD"),
        &signature,
        &signature,
        &input.message,
        &tree,
        &parents,
    )?;
    Ok(GitCommit {
        head: head.to_string(),
    })
}

fn push(repo: &Repository, input: &PushGit) -> Result<GitPush, RpcError> {
    let head = repo.head()?;
    if !head.is_branch() {
        return Err(RpcError::invalid("check out a local branch before pushing"));
    }
    let source = head.name()?.to_owned();
    let upstream = repo.branch_upstream_remote(&source).ok();
    let upstream = upstream.as_ref().map(git2::Buf::as_str).transpose()?;
    let remote = input.remote.as_deref().or(upstream).unwrap_or("origin");
    let merge = if upstream == Some(remote) {
        repo.branch_upstream_merge(&source).ok()
    } else {
        None
    };
    let reference = merge
        .as_ref()
        .map(git2::Buf::as_str)
        .transpose()?
        .unwrap_or(&source);
    if !reference.starts_with("refs/heads/") {
        return Err(RpcError::invalid("the push destination must be a branch"));
    }
    let mut destination = repo.find_remote(remote)?;
    let config = repo.config()?;
    let mut callbacks = RemoteCallbacks::new();
    let mut tried_credential = false;
    callbacks.credentials(|url, username, methods| {
        if methods.contains(CredentialType::USERNAME) {
            return Cred::username(username.unwrap_or("git"));
        }
        if std::mem::replace(&mut tried_credential, true) {
            return Err(git2::Error::from_str(
                "Git remote rejected the available credential",
            ));
        }
        if methods.contains(CredentialType::SSH_KEY) {
            Cred::ssh_key_from_agent(username.unwrap_or("git"))
        } else if methods.contains(CredentialType::USER_PASS_PLAINTEXT) {
            Cred::credential_helper(&config, url, username)
        } else {
            Cred::default()
        }
    });
    callbacks.push_update_reference(|_, status| {
        status.map_or(Ok(()), |message| Err(git2::Error::from_str(message)))
    });
    let mut options = PushOptions::new();
    options.remote_callbacks(callbacks);
    destination.push(&[format!("{source}:{reference}")], Some(&mut options))?;
    Ok(GitPush {
        remote: remote.to_owned(),
        reference: reference.to_owned(),
    })
}
