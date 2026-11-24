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
    sql::{self, Conn, exec, row},
    store::{self, NewThread, SavedSession},
};
use crate::{acp, error::Error, wire::NewEntry};

impl Workspace {
    pub(super) async fn harness_sessions(
        self: &Arc<Self>,
        input: ListHarnessSessions,
    ) -> Result<HarnessSessions, Error> {
        let (harness, config) = self.harness(Some(&input.harness))?;
        let harness = harness.to_owned();
        let project = input.project.clone();
        let ws = Arc::clone(self);
        let checkout = self
            .db
            .read(move |conn| ws.checkout_for(conn, project.as_deref()))
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
        let sessions = self
            .db
            .read_snapshot(move |tx| {
                let mut sessions = Vec::with_capacity(saved.len());
                for session in saved {
                    sessions.push(HarnessSession {
                        thread: bound(tx, &harness, &session.id)?,
                        cwd: session.cwd.display().to_string(),
                        id: session.id,
                        title: session.title,
                        updated_at: session.updated_at,
                    });
                }
                Ok(sessions)
            })
            .await?;
        Ok(HarnessSessions {
            supported: true,
            sessions,
            next,
        })
    }

    /// Imports a session, or returns the thread already bound to it.
    pub(super) async fn import(self: &Arc<Self>, input: ImportSession) -> Result<Launched, Error> {
        let (harness, config) = self.harness(Some(&input.harness))?;
        let harness_name = harness.to_owned();
        let session_id = input.session.clone();
        let project = input.project.clone();
        let ws = Arc::clone(self);
        let precheck = self
            .db
            .read_snapshot(move |tx| {
                if let Some(thread) = bound(tx, &harness_name, &session_id)? {
                    return Ok(Err(Launched { thread, run: None }));
                }
                Ok(Ok(ws.checkout_for(tx, project.as_deref())?))
            })
            .await?;
        let checkout = match precheck {
            Ok(checkout) => checkout,
            Err(launched) => return Ok(launched),
        };
        let turns = acp::load_history(config, Path::new(&checkout.cwd), &input.session, "import")
            .await?
            .unwrap_or_default();
        let harness_name = harness.to_owned();
        let ws = Arc::clone(self);
        let thread = self
            .db
            .write(move |conn| {
                let tx = sql::immediate(conn)?;
                let title = input
                    .title
                    .as_deref()
                    .or_else(|| turns.first().map(|turn| turn.prompt.as_str()));
                let thread = store::create_thread(
                    &ws.reservations,
                    &tx,
                    &NewThread {
                        harness: &harness_name,
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
                )?;
                for turn in turns {
                    let prompt = store::append(
                        &tx,
                        thread,
                        &NewEntry::Prompt {
                            text: turn.prompt.clone(),
                            attachments: Vec::new(),
                            context: Vec::new(),
                            origin: None,
                            truncated: false,
                        },
                    )?;
                    for entry in turn.entries {
                        store::publish(&tx, thread, &input.session, entry, &ws.entry_chunks)?;
                    }
                    let end = store::append(
                        &tx,
                        thread,
                        &NewEntry::Run(RunEnd {
                            stop_reason: Some("end_turn".to_owned()),
                            error: None,
                            answer: Some(turn.answer.clone()),
                            usage: None,
                        }),
                    )?;
                    exec(
                        &tx,
                        "INSERT INTO runs (thread, kind, status, position, text, prompt_entry, end_entry, answer)
                        VALUES (?1, 'prompt', 'completed', 0, ?2, ?3, ?4, ?5)",
                        [
                            thread.into(),
                            turn.prompt.into(),
                            prompt.into(),
                            end.into(),
                            turn.answer.into(),
                        ],
                    )?;
                }
                tx.commit()?;
                ws.kick(thread, true);
                Ok(thread)
            })
            .await?;
        self.relist(thread);
        Ok(Launched { thread, run: None })
    }
}

/// The thread bound to a harness's session, if any.
fn bound(db: &Conn, harness: &str, session: &str) -> Result<Option<viz_protocol::ThreadId>, Error> {
    row(
        db,
        "SELECT id FROM threads WHERE harness = ?1 AND json_extract(session, '$.session_id') = ?2",
        [harness.into(), session.into()],
    )?
    .map(|row| row.id(0))
    .transpose()
}
