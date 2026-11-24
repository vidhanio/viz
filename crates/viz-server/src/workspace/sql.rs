//! Synchronous SQLite writer actor paired with a bounded WAL read pool.
//!
//! One dedicated OS thread owns the sole read/write SQLite connection so every
//! write closure runs synchronously to completion without ever holding a
//! transaction across an `.await`. Concurrent read queries run on a small pool
//! of `SQLITE_OPEN_READ_ONLY` WAL connections.

mod migrations;

use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    thread,
};

use rusqlite::{
    Transaction, TransactionBehavior, params_from_iter,
    types::{ToSql, ToSqlOutput, Value as SqlValue},
};
use tokio::sync::{Semaphore, mpsc, oneshot};
use viz_protocol::{EntryId, RequestId, RunId, TaskId, ThreadId};

use crate::error::Error;

/// Underlying synchronous `rusqlite` connection passed to read and write
/// closures (and dereferenced from `rusqlite::Transaction`).
pub(crate) type Conn = rusqlite::Connection;

const READ_POOL_SIZE: usize = 4;

type WriteJob = Box<dyn FnOnce(&mut Conn) + Send>;

#[derive(Debug)]
struct ReadPool {
    semaphore: Semaphore,
    connections: Mutex<Vec<Conn>>,
}

struct PooledReader {
    pool: Arc<ReadPool>,
    conn: Option<Conn>,
}

impl PooledReader {
    const fn conn_mut(&mut self) -> &mut Conn {
        self.conn
            .as_mut()
            .expect("pooled SQLite reader present until drop")
    }
}

impl Drop for PooledReader {
    fn drop(&mut self) {
        if let Some(conn) = self.conn.take() {
            let mut connections = self
                .pool
                .connections
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            connections.push(conn);
        }
    }
}

/// Concurrent WAL read pool + serialized synchronous writer actor.
#[derive(Debug)]
pub(crate) struct Database {
    writer: mpsc::UnboundedSender<WriteJob>,
    readers: Arc<ReadPool>,
}

impl Database {
    pub(super) async fn open(path: PathBuf) -> anyhow::Result<Self> {
        let (writer, mut inbox) = mpsc::unbounded_channel::<WriteJob>();
        let (ready, initialized) = oneshot::channel();
        let writer_path = path.clone();
        let runtime = tokio::runtime::Handle::current();
        thread::Builder::new()
            .name("viz-sqlite".into())
            .spawn(move || {
                let _runtime = runtime.enter();
                let mut db = match migrations::open(&writer_path) {
                    Ok(db) => {
                        if ready.send(Ok(())).is_err() {
                            return;
                        }
                        db
                    }
                    Err(error) => {
                        let _ = ready.send(Err(error));
                        return;
                    }
                };
                while let Some(job) = inbox.blocking_recv() {
                    job(&mut db);
                    if !db.is_autocommit()
                        && let Err(error) = db.execute_batch("ROLLBACK")
                    {
                        tracing::error!(%error, "rolling back leaked workspace transaction");
                        break;
                    }
                }
            })?;
        initialized.await??;
        let reader_conns = tokio::task::spawn_blocking(move || {
            (0..READ_POOL_SIZE)
                .map(|_| migrations::open_reader(&path))
                .collect::<anyhow::Result<Vec<_>>>()
        })
        .await??;
        Ok(Self {
            writer,
            readers: Arc::new(ReadPool {
                semaphore: Semaphore::new(READ_POOL_SIZE),
                connections: Mutex::new(reader_conns),
            }),
        })
    }

    /// Executes a synchronous read-only query closure on the WAL reader pool.
    pub(crate) async fn read<T: Send + 'static>(
        &self,
        operation: impl FnOnce(&mut Conn) -> Result<T, Error> + Send + 'static,
    ) -> Result<T, Error> {
        let _permit = self
            .readers
            .semaphore
            .acquire()
            .await
            .map_err(|_| Error::WriterStopped)?;
        let conn = self
            .readers
            .connections
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pop()
            .ok_or(Error::WriterStopped)?;
        let mut lease = PooledReader {
            pool: Arc::clone(&self.readers),
            conn: Some(conn),
        };
        tokio::task::spawn_blocking(move || operation(lease.conn_mut()))
            .await
            .map_err(|_| Error::WriterStopped)?
    }

    /// Executes a synchronous multi-query read snapshot inside a `DEFERRED`
    /// read transaction on the WAL reader pool so every query sees the same
    /// committed state.
    pub(crate) async fn read_snapshot<T: Send + 'static>(
        &self,
        operation: impl FnOnce(&Transaction<'_>) -> Result<T, Error> + Send + 'static,
    ) -> Result<T, Error> {
        self.read(move |conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Deferred)?;
            operation(&tx)
        })
        .await
    }

    /// Executes a synchronous write closure on the dedicated SQLite writer
    /// thread. Use this when a post-commit in-memory update must serialize
    /// before the next write begins.
    pub(crate) async fn write<T: Send + 'static>(
        &self,
        operation: impl FnOnce(&mut Conn) -> Result<T, Error> + Send + 'static,
    ) -> Result<T, Error> {
        let (reply, result) = oneshot::channel();
        self.writer
            .send(Box::new(move |db| {
                let _ = reply.send(operation(db));
            }))
            .map_err(|_| Error::WriterStopped)?;
        result.await.map_err(|_| Error::WriterStopped)?
    }

    /// Executes a synchronous write closure inside an `IMMEDIATE` transaction
    /// on the dedicated SQLite writer thread, committing on `Ok` and rolling
    /// back automatically on `Err`.
    pub(crate) async fn write_tx<T: Send + 'static>(
        &self,
        operation: impl FnOnce(&Transaction<'_>) -> Result<T, Error> + Send + 'static,
    ) -> Result<T, Error> {
        self.write(move |conn| {
            let tx = immediate(conn)?;
            let value = operation(&tx)?;
            tx.commit()?;
            Ok(value)
        })
        .await
    }
}

/// Starts an `IMMEDIATE` write transaction inside a [`Database::write`]
/// closure.
pub(crate) fn immediate(conn: &mut Conn) -> Result<Transaction<'_>, Error> {
    Ok(conn.transaction_with_behavior(TransactionBehavior::Immediate)?)
}

#[derive(Debug)]
pub(crate) enum Value {
    Sql(SqlValue),
    Unsigned(u64),
}

impl ToSql for Value {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        match self {
            Self::Sql(value) => value.to_sql(),
            Self::Unsigned(value) => i64::try_from(*value)
                .map(|value| ToSqlOutput::Owned(SqlValue::Integer(value)))
                .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error))),
        }
    }
}

macro_rules! sql_values {
    ($($type:ty => $variant:ident, $convert:expr;)*) => {
        $(impl From<$type> for Value {
            fn from(value: $type) -> Self {
                Self::Sql(SqlValue::$variant(($convert)(value)))
            }
        })*
    };
}

sql_values! {
    bool => Integer, i64::from;
    i64 => Integer, std::convert::identity;
    String => Text, std::convert::identity;
    &str => Text, str::to_owned;
}

impl From<u64> for Value {
    fn from(value: u64) -> Self {
        Self::Unsigned(value)
    }
}

/// Numeric identifiers stored as SQLite integers.
pub(crate) trait Id {
    fn from_column(value: u64) -> Self;
}

macro_rules! sql_ids {
    ($($id:ident),+) => {$(
        impl Id for $id {
            fn from_column(value: u64) -> Self {
                Self(value)
            }
        }

        impl From<$id> for Value {
            fn from(id: $id) -> Self {
                Self::Unsigned(id.0)
            }
        }
    )+};
}

sql_ids!(ThreadId, RunId, EntryId, RequestId);

impl From<TaskId> for Value {
    fn from(id: TaskId) -> Self {
        id.0.into()
    }
}

impl From<&TaskId> for Value {
    fn from(id: &TaskId) -> Self {
        id.as_str().into()
    }
}

impl From<usize> for Value {
    fn from(value: usize) -> Self {
        Self::Unsigned(value as u64)
    }
}

impl<T: Into<Self>> From<Option<T>> for Value {
    fn from(value: Option<T>) -> Self {
        value.map_or(Self::Sql(SqlValue::Null), Into::into)
    }
}

pub(crate) fn exec<const N: usize>(
    db: &Conn,
    statement: &str,
    args: [Value; N],
) -> Result<usize, Error> {
    Ok(db
        .prepare_cached(statement)?
        .execute(params_from_iter(args))?)
}

pub(crate) fn rows<const N: usize>(
    db: &Conn,
    query: &str,
    args: [Value; N],
) -> Result<std::vec::IntoIter<Row>, Error> {
    let mut statement = db.prepare_cached(query)?;
    let columns = statement.column_count();
    let collected = statement
        .query_map(params_from_iter(args), |row| {
            (0..columns)
                .map(|column| row.get(column))
                .collect::<rusqlite::Result<Vec<_>>>()
                .map(Row)
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(collected.into_iter())
}

pub(crate) fn row<const N: usize>(
    db: &Conn,
    query: &str,
    args: [Value; N],
) -> Result<Option<Row>, Error> {
    Ok(rows(db, query, args)?.next())
}

#[derive(Debug)]
pub(crate) struct Row(Vec<SqlValue>);

impl Row {
    fn take(&mut self, col: usize) -> SqlValue {
        std::mem::replace(&mut self.0[col], SqlValue::Null)
    }

    pub(crate) fn str(&mut self, col: usize) -> Result<String, Error> {
        self.opt_str(col)?.ok_or_else(|| invalid_column(col))
    }

    pub(crate) fn opt_str(&mut self, col: usize) -> Result<Option<String>, Error> {
        match self.take(col) {
            SqlValue::Text(value) => Ok(Some(value)),
            SqlValue::Null => Ok(None),
            _ => Err(invalid_column(col)),
        }
    }

    pub(crate) fn i64(&self, col: usize) -> Result<i64, Error> {
        self.opt_i64(col)?.ok_or_else(|| invalid_column(col))
    }

    pub(crate) fn opt_i64(&self, col: usize) -> Result<Option<i64>, Error> {
        match self.0[col] {
            SqlValue::Integer(value) => Ok(Some(value)),
            SqlValue::Null => Ok(None),
            _ => Err(invalid_column(col)),
        }
    }

    pub(crate) fn u64(&self, col: usize) -> Result<u64, Error> {
        self.opt_u64(col)?.ok_or_else(|| invalid_column(col))
    }

    pub(crate) fn opt_u64(&self, col: usize) -> Result<Option<u64>, Error> {
        match self.0[col] {
            SqlValue::Integer(value) => u64::try_from(value)
                .map(Some)
                .map_err(|_| invalid_column(col)),
            SqlValue::Null => Ok(None),
            _ => Err(invalid_column(col)),
        }
    }

    pub(crate) fn id<T: Id>(&self, col: usize) -> Result<T, Error> {
        self.u64(col).map(T::from_column)
    }

    pub(crate) fn opt_id<T: Id>(&self, col: usize) -> Result<Option<T>, Error> {
        Ok(self.opt_u64(col)?.map(T::from_column))
    }

    pub(crate) fn flag(&self, col: usize) -> Result<bool, Error> {
        Ok(self.i64(col)? != 0)
    }
}

fn invalid_column(col: usize) -> Error {
    Error::corrupt(format_args!("unexpected SQLite value in column {col}"))
}
