//! Open and configure the worker's ACP session and persist its latest view.
//! Session notifications are read by the worker that owns the process.

use std::path::Path;

use tokio::sync::watch;
use tokio_util::sync::CancellationToken;
use viz_core::{options::Labels, titles};
use viz_protocol::ThreadId;

use super::{
    Workspace,
    sql::{Connection, exec, row},
    store::{self, Binding, Handoff},
};
use crate::{
    acp::{Catalog, Desired, Housekeeping, OpenSession, Session, SessionSnapshot, housekeep},
    error::Error,
    wire::OptionValue,
};

#[derive(Default)]
pub(super) struct SessionSlot {
    pub(super) current: Option<Session>,
    updates: Option<watch::Receiver<()>>,
}

impl SessionSlot {
    pub(super) fn clear(&mut self) {
        self.updates.take();
        self.current.take();
    }

    /// Releases the session, telling the harness it is closed here.
    pub(super) async fn close(&mut self) {
        self.updates.take();
        if let Some(session) = self.current.take() {
            session.close().await;
        }
    }

    pub(super) fn is_restorable(&self) -> bool {
        self.current.as_ref().is_some_and(Session::is_restorable)
    }

    pub(super) async fn changed(&mut self) -> bool {
        let Some(updates) = &mut self.updates else {
            return std::future::pending().await;
        };
        if updates.changed().await.is_ok() {
            true
        } else {
            self.updates.take();
            false
        }
    }
}

impl Workspace {
    pub(super) async fn session<'slot>(
        &self,
        thread: ThreadId,
        slot: &'slot mut SessionSlot,
        cancel: &CancellationToken,
    ) -> Result<Bound<'slot>, Error> {
        let Binding {
            harness,
            checkout,
            session,
            project,
            ..
        } = {
            let mut db = self.db.lock().await;
            store::binding(&mut db, thread).await?
        };
        if slot.current.as_ref().is_some_and(Session::is_alive) {
            return Ok(Bound {
                session: slot.current.as_mut().expect("checked live session"),
                harness,
                desired: session.desired,
            });
        }
        slot.clear();
        let (_, config) = self.harness(Some(&harness))?;
        let endpoint = self.mcp.endpoint(thread);
        let mut setup = viz_acp::SessionSetup::new(&checkout.cwd);
        if let Some(project) = project {
            let (_, _, prefs) = store::preferences(&mut *self.db.lock().await, &project).await?;
            setup.additional_directories = prefs
                .settings
                .additional_directories
                .into_iter()
                .map(Into::into)
                .collect();
        }
        let operation = match (session.session_id.as_deref(), &session.handoff) {
            (Some(id), _) => OpenSession::Restore {
                id,
                replay: Some(""),
            },
            (None, Some(Handoff::Forked { session: Some(id) })) => OpenSession::Fork {
                id,
                replay: Some(""),
            },
            (None, _) => OpenSession::New,
        };
        let mut running = tokio::select! {
            biased;
            () = cancel.cancelled() => return Err(Error::from(viz_acp::Error::Cancelled)),
            result = Session::open(config, setup, operation, Some(&endpoint), self.client_events(thread)) => result?,
        };
        let fell_back = running.take_handoff().is_some();
        tracing::info!(%thread, session = running.id(), ?operation, fell_back, "opened ACP session");
        let handoff = match operation {
            OpenSession::Restore { .. } if fell_back => Some(Some(Handoff::Reopened)),
            OpenSession::Fork { .. } if fell_back => Some(Some(Handoff::Forked { session: None })),
            OpenSession::Fork { .. } => Some(None),
            OpenSession::Restore { .. } | OpenSession::New => None,
        };
        if session.session_id.as_deref() != Some(running.id()) || handoff.is_some() {
            let handoff = handoff.unwrap_or(session.handoff);
            exec(
                &mut *self.db.lock().await,
                "UPDATE threads SET session = json_set(session, '$.session_id', ?2, '$.handoff', json(?3)) WHERE id = ?1",
                [thread.into(), running.id().into(), store::json(&handoff)?.into()],
            )
            .await?;
        }
        let desired = tokio::select! {
            biased;
            () = cancel.cancelled() => return Err(Error::from(viz_acp::Error::Cancelled)),
            result = running.restore(&session.desired, session.selection.as_ref()) => result?,
        };
        let desired = self.apply_access(thread, &running, desired).await?;
        let updates = running.watch();
        let relist = {
            let mut db = self.db.lock().await;
            let mut tx = db.transaction().await?;
            let snapshot = running.snapshot();
            update_catalog(&mut tx, &harness, &snapshot).await?;
            let relist = persist_snapshot(&mut tx, thread, snapshot, Some(&desired)).await?;
            tx.commit().await?;
            drop(db);
            relist
        };
        slot.updates = Some(updates);
        let session_ref = slot.current.insert(running);
        if relist {
            self.relist(thread);
        } else {
            self.changed(Some(thread));
        }
        Ok(Bound {
            session: session_ref,
            harness,
            desired,
        })
    }

    /// Switches a newly opened session to the thread's or project's chosen
    /// harness mode. A harness that refuses keeps its own mode.
    async fn apply_access(
        &self,
        thread: ThreadId,
        session: &Session,
        desired: Desired,
    ) -> Result<Desired, Error> {
        let (chosen, project) = store::chosen_access(&mut *self.db.lock().await, thread).await?;
        let Some(native) = chosen.or(project).and_then(|mode| mode.native) else {
            return Ok(desired);
        };
        let value = OptionValue::Select(native.value);
        let current = session.snapshot().options;
        if current
            .iter()
            .any(|option| option.id == native.option && option.value == value)
        {
            return Ok(desired);
        }
        match session.configure(&native.option, value, &desired).await {
            Ok(desired) => Ok(desired),
            Err(error) => {
                tracing::warn!(%thread, %error, "applying the chosen access mode");
                Ok(desired)
            }
        }
    }

    pub(super) async fn configure_session(
        &self,
        thread: ThreadId,
        slot: &mut SessionSlot,
        cancel: &CancellationToken,
        option: &str,
        value: OptionValue,
    ) -> Result<(), Error> {
        let Bound {
            session,
            harness,
            desired,
        } = self.session(thread, slot, cancel).await?;
        let desired = tokio::select! {
            biased;
            () = cancel.cancelled() => return Err(Error::unavailable("ACP configuration cancelled")),
            result = session.configure(option, value, &desired) => result.map_err(|error| match error {
                viz_acp::Error::InvalidOption(_) => Error::invalid(error.to_string()),
                other => other.into(),
            })?,
        };
        let snapshot = session.snapshot();
        let mut db = self.db.lock().await;
        let mut tx = db.transaction().await?;
        update_catalog(&mut tx, &harness, &snapshot).await?;
        let relist = persist_snapshot(&mut tx, thread, snapshot, Some(&desired)).await?;
        tx.commit().await?;
        drop(db);
        if relist {
            self.relist(thread);
        } else {
            self.changed(Some(thread));
        }
        Ok(())
    }

    pub(super) async fn refresh_session(
        &self,
        thread: ThreadId,
        slot: &SessionSlot,
    ) -> Result<(), Error> {
        let Some(session) = slot.current.as_ref() else {
            return Ok(());
        };
        let updated =
            persist_snapshot(&mut *self.db.lock().await, thread, session.snapshot(), None).await?;
        if updated {
            self.relist(thread);
        } else {
            self.changed(Some(thread));
        }
        Ok(())
    }
}

impl Workspace {
    /// Deletes or archives a thread's saved harness session in the
    /// background, where the harness supports it.
    pub(super) fn housekeep(&self, binding: &Binding, command: Housekeeping) {
        let (Some(id), Some(config)) = (
            binding.session.session_id.clone(),
            self.harnesses.get(&binding.harness).cloned(),
        ) else {
            return;
        };
        let cwd = binding.checkout.cwd.clone();
        self.jobs.spawn(async move {
            match housekeep(&config, Path::new(&cwd), &id, command).await {
                Ok(done) => {
                    tracing::debug!(session = %id, ?command, done, "ACP session housekeeping");
                }
                Err(error) => {
                    tracing::warn!(session = %id, ?command, %error, "ACP session housekeeping");
                }
            }
        });
    }

    /// Names the thread's open session after the user's title.
    pub(super) async fn title_session(&self, thread: ThreadId, slot: &SessionSlot, title: &str) {
        let Some(session) = slot.current.as_ref().filter(|session| session.is_alive()) else {
            return;
        };
        if let Err(error) = session.set_title(title).await {
            tracing::warn!(%thread, %error, "naming the ACP session");
        }
    }
}

pub(super) struct Bound<'slot> {
    pub(super) session: &'slot mut Session,
    harness: String,
    desired: Desired,
}

pub(super) async fn catalog(db: &mut Connection, name: &str) -> Result<Option<Catalog>, Error> {
    row(
        db,
        "SELECT data FROM harness_catalogs WHERE name = ?1",
        [name.into()],
    )
    .await?
    .map(|mut row| store::from_json(&row.str(0)?))
    .transpose()
}

pub(super) async fn save_catalog(
    db: &mut Connection,
    name: &str,
    catalog: &Catalog,
) -> Result<(), Error> {
    exec(db, "INSERT INTO harness_catalogs (name, data) VALUES (?1, ?2) ON CONFLICT(name) DO UPDATE SET data = excluded.data WHERE data <> excluded.data",
        [name.into(), store::json(catalog)?.into()]).await?;
    Ok(())
}

async fn update_catalog(
    db: &mut Connection,
    name: &str,
    snapshot: &SessionSnapshot,
) -> Result<(), Error> {
    let stored = catalog(db, name).await?;
    if let Some(updated) = Catalog::merge(stored.as_ref(), &snapshot.options, &snapshot.opened) {
        save_catalog(db, name, &updated).await?;
    }
    Ok(())
}

pub(super) async fn persist_snapshot(
    db: &mut Connection,
    thread: ThreadId,
    snapshot: SessionSnapshot,
    desired: Option<&Desired>,
) -> Result<bool, Error> {
    let commands: Vec<_> = snapshot
        .commands
        .into_iter()
        .map(|command| viz_protocol::SlashCommand {
            name: command.name,
            description: command.description,
            hint: command.hint,
        })
        .collect();
    let usage = snapshot.usage.map(|usage| viz_protocol::ContextUsage {
        used: usage.used,
        size: usage.size,
    });
    let Labels { model, thinking } = Labels::from_options(&snapshot.options);
    let Some(mut prior) = row(
        db,
        "SELECT title, user_title, model, thinking, setup_failed FROM threads WHERE id = ?1",
        [thread.into()],
    )
    .await?
    else {
        return Ok(false);
    };
    let prior_title = prior.str(0)?;
    let title = titles::observed(
        prior_title.clone(),
        prior.flag(1)?,
        snapshot.title.as_deref(),
    );
    let relist = title != prior_title
        || prior.opt_str(2)?.as_deref() != model
        || prior.opt_str(3)?.as_deref() != thinking
        || prior.flag(4)?;
    exec(db, "UPDATE threads SET
          session = json_set(session, '$.session_id', ?2, '$.options', json(?3), '$.commands', json(?4), '$.images', json(?5), '$.usage', json(?6),
            '$.title', coalesce(?10, json_extract(session, '$.title'))),
          title = ?7,
          model = ?8, thinking = ?9, setup_failed = 0 WHERE id = ?1", [
        thread.into(), snapshot.session_id.into(), store::json(&snapshot.options)?.into(), store::json(&commands)?.into(),
        if snapshot.images { "true" } else { "false" }.into(), store::json(&usage)?.into(), title.into(), model.into(), thinking.into(),
        snapshot.title.into(),
    ]).await?;
    if let Some(desired) = desired {
        exec(db, "UPDATE threads SET session = json_set(session, '$.desired', json(?2), '$.selection', NULL) WHERE id = ?1",
            [thread.into(), store::json(desired)?.into()]).await?;
    }
    Ok(relist)
}
