//! Open a harness's saved sessions as threads.
//!
//! The harness lists the sessions it saved for a directory. Importing one
//! loads it in a disposable process and records the history it replays as
//! finished runs, so the thread reads, forks and hands off like one viz ran
//! from the start. The thread then resumes the same session.

use std::{path::Path, sync::Arc};

use viz_protocol::{
    HarnessSession, HarnessSessions, ImportSession, Launched, ListHarnessSessions, RunEnd,
};

use super::{
    Workspace,
    sql::{Connection, exec, row},
    store::{self, NewThread, SavedSession},
};
use crate::{acp, error::Error, wire::NewEntry};

impl Workspace {
    pub(super) async fn harness_sessions(
        &self,
        input: ListHarnessSessions,
    ) -> Result<HarnessSessions, Error> {
        let (harness, config) = self.harness(Some(&input.harness))?;
        let checkout = self
            .checkout_for(&mut *self.db.lock().await, input.project.as_deref())
            .await?;
        let Some((saved, next)) =
            acp::saved_sessions(config, Path::new(&checkout.cwd), input.cursor).await?
        else {
            return Ok(HarnessSessions {
                supported: false,
                sessions: Vec::new(),
                next: None,
            });
        };
        let mut db = self.db.lock().await;
        let mut sessions = Vec::with_capacity(saved.len());
        for session in saved {
            sessions.push(HarnessSession {
                thread: bound(&mut db, harness, &session.id).await?,
                cwd: session.cwd.display().to_string(),
                id: session.id,
                title: session.title,
                updated_at: session.updated_at,
            });
        }
        Ok(HarnessSessions {
            supported: true,
            sessions,
            next,
        })
    }

    /// Imports a session, or returns the thread already bound to it.
    pub(super) async fn import(self: &Arc<Self>, input: ImportSession) -> Result<Launched, Error> {
        let (harness, config) = self.harness(Some(&input.harness))?;
        let checkout = {
            let mut db = self.db.lock().await;
            if let Some(thread) = bound(&mut db, harness, &input.session).await? {
                return Ok(Launched { thread, run: None });
            }
            self.checkout_for(&mut db, input.project.as_deref()).await?
        };
        let turns = acp::load_history(config, Path::new(&checkout.cwd), &input.session, "import")
            .await?
            .unwrap_or_default();
        let title = input
            .title
            .as_deref()
            .or_else(|| turns.first().map(|turn| turn.prompt.as_str()));
        let mut db = self.db.lock().await;
        let mut tx = db.transaction().await?;
        let thread = store::create_thread(
            &mut tx,
            &NewThread {
                harness,
                checkout: &checkout,
                project: input.project.as_deref(),
                parent: None,
                parent_environment: None,
                title,
                session: SavedSession {
                    session_id: Some(input.session.clone()),
                    ..SavedSession::default()
                },
            },
        )
        .await?;
        for turn in turns {
            let prompt = store::append(
                &mut tx,
                thread,
                &NewEntry::Prompt {
                    text: turn.prompt.clone(),
                    attachments: Vec::new(),
                    context: Vec::new(),
                    origin: None,
                },
            )
            .await?;
            for entry in turn.entries {
                store::publish(&mut tx, thread, &input.session, entry).await?;
            }
            let end = store::append(
                &mut tx,
                thread,
                &NewEntry::Run(RunEnd {
                    stop_reason: Some("end_turn".to_owned()),
                    error: None,
                    answer: Some(turn.answer.clone()),
                    usage: None,
                }),
            )
            .await?;
            exec(
                &mut tx,
                "INSERT INTO runs (thread, kind, status, position, text, prompt_entry, end_entry, answer)
                VALUES (?1, 'prompt', 'completed', 0, ?2, ?3, ?4, ?5)",
                [
                    thread.into(),
                    turn.prompt.into(),
                    prompt.into(),
                    end.into(),
                    turn.answer.into(),
                ],
            )
            .await?;
        }
        tx.commit().await?;
        self.kick(thread, true);
        drop(db);
        self.relist(thread);
        Ok(Launched { thread, run: None })
    }
}

/// The thread bound to a harness's session, if any.
async fn bound(
    db: &mut Connection,
    harness: &str,
    session: &str,
) -> Result<Option<viz_protocol::ThreadId>, Error> {
    row(
        db,
        "SELECT id FROM threads WHERE harness = ?1 AND json_extract(session, '$.session_id') = ?2",
        [harness.into(), session.into()],
    )
    .await?
    .map(|row| row.id(0))
    .transpose()
}
