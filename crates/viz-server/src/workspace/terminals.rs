//! Resolves terminal ownership against the environment's workspace.

use tokio::sync::broadcast;
use viz_protocol::{
    CreateTerminal, InteractiveScope, ListTerminals, TerminalFilter, TerminalId, TerminalList,
    TerminalScope, TerminalSummary, ThreadId,
};
use viz_terminal::pty::{Launch, Program, TerminalHandle};

use super::{Workspace, sql::rows, store};
use crate::error::Error;

impl Workspace {
    pub(crate) async fn terminal_changes(&self) -> broadcast::Receiver<()> {
        self.terminals.lock().await.subscribe()
    }

    pub(crate) async fn terminal(&self, id: TerminalId) -> Result<TerminalHandle, Error> {
        self.terminals
            .lock()
            .await
            .get(id)
            .cloned()
            .ok_or_else(|| no_terminal(id))
    }

    pub(super) async fn create_terminal(
        &self,
        args: CreateTerminal,
    ) -> Result<TerminalSummary, Error> {
        let mut db = self.db.lock().await;
        let (scope, cwd) = match args.scope {
            InteractiveScope::Thread { thread } => {
                let binding = store::binding(&mut db, thread).await?;
                (TerminalScope::Thread { thread }, binding.checkout.cwd)
            }
            InteractiveScope::Project { project } => {
                let project = store::project(&mut db, &project).await?.path;
                (
                    TerminalScope::Project {
                        project: project.clone(),
                    },
                    project,
                )
            }
        };
        let (summary, _) = self
            .terminals
            .lock()
            .await
            .create(
                scope,
                Launch {
                    program: Program::LoginShell,
                    cwd: cwd.into(),
                    env: Vec::new(),
                    size: args.size,
                    cell: args.cell,
                },
            )
            .await?;
        drop(db);
        Ok(summary)
    }

    pub(super) async fn list_terminals(&self, args: ListTerminals) -> Result<TerminalList, Error> {
        let mut db = self.db.lock().await;
        let filed = match &args.filter {
            Some(TerminalFilter::Project { project }) => project_threads(&mut db, project).await?,
            _ => Vec::new(),
        };
        let mut terminals: Vec<_> = self
            .terminals
            .lock()
            .await
            .summaries()
            .filter(|summary| match &args.filter {
                None => true,
                Some(TerminalFilter::Thread { thread }) => summary.scope.thread() == Some(*thread),
                Some(TerminalFilter::Project { project }) => match &summary.scope {
                    TerminalScope::Project { project: owner } => owner == project,
                    scope => scope.thread().is_some_and(|thread| filed.contains(&thread)),
                },
            })
            .collect();
        drop(db);
        terminals.sort_unstable_by_key(|terminal| (terminal.created_at, terminal.id));
        Ok(TerminalList { terminals })
    }

    pub(super) async fn close_terminal(&self, id: TerminalId) -> Result<(), Error> {
        let mut terminals = self.terminals.lock().await;
        if terminals.get(id).is_none() {
            return Err(no_terminal(id));
        }
        terminals.release(id).await;
        drop(terminals);
        Ok(())
    }
}

pub(super) async fn project_threads(
    db: &mut super::sql::Connection,
    project: &str,
) -> Result<Vec<ThreadId>, Error> {
    rows(
        db,
        "SELECT id FROM threads WHERE project = ?1",
        [project.into()],
    )
    .await?
    .map(|row| row.id(0))
    .collect()
}

fn no_terminal(id: TerminalId) -> Error {
    Error::not_found(format!("no terminal {}", id.0))
}
