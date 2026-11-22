//! A dedicated writer thread owns the SQLite connection. The workspace leases
//! this handle across each read/decide/write group. Dropping an uncommitted
//! transaction queues rollback before the next lease can submit work.

mod migrations;

use std::{
    ops::{Deref, DerefMut},
    path::PathBuf,
    thread,
};

use rusqlite::{
    Connection as Sqlite, params_from_iter,
    types::{ToSql, ToSqlOutput, Value as SqlValue},
};
use tokio::sync::{mpsc, oneshot};

use crate::error::Error;

type Job = Box<dyn FnOnce(&mut Sqlite) + Send>;

enum Message {
    Work(Job),
    Rollback,
}

#[derive(Debug)]
pub(crate) struct Connection {
    writer: mpsc::UnboundedSender<Message>,
}

impl Connection {
    pub(super) async fn open(path: PathBuf) -> anyhow::Result<Self> {
        let (writer, mut inbox) = mpsc::unbounded_channel();
        let (ready, initialized) = oneshot::channel();
        thread::Builder::new()
            .name("viz-sqlite".into())
            .spawn(move || {
                let mut db = match migrations::open(&path) {
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
                while let Some(message) = inbox.blocking_recv() {
                    match message {
                        Message::Work(job) => job(&mut db),
                        Message::Rollback if !db.is_autocommit() => {
                            if let Err(error) = db.execute_batch("ROLLBACK") {
                                tracing::error!(%error, "rolling back workspace transaction");
                                break;
                            }
                        }
                        Message::Rollback => {}
                    }
                }
            })?;
        initialized.await??;
        Ok(Self { writer })
    }

    #[expect(
        clippy::needless_pass_by_ref_mut,
        reason = "the exclusive lease serializes transaction groups"
    )]
    async fn call<T: Send + 'static>(
        &mut self,
        operation: impl FnOnce(&mut Sqlite) -> rusqlite::Result<T> + Send + 'static,
    ) -> Result<T, Error> {
        let (reply, result) = oneshot::channel();
        self.writer
            .send(Message::Work(Box::new(move |db| {
                let _ = reply.send(operation(db));
            })))
            .map_err(|_| Error::WriterStopped)?;
        result
            .await
            .map_err(|_| Error::WriterStopped)?
            .map_err(Error::Storage)
    }

    pub(crate) async fn transaction(&mut self) -> Result<Transaction<'_>, Error> {
        let tx = Transaction {
            db: self,
            committed: false,
        };
        tx.db.call(|db| db.execute_batch("BEGIN IMMEDIATE")).await?;
        Ok(tx)
    }
}

#[derive(Debug)]
pub(crate) struct Transaction<'a> {
    db: &'a mut Connection,
    committed: bool,
}

impl Transaction<'_> {
    pub(crate) async fn commit(mut self) -> Result<(), Error> {
        self.db.call(|db| db.execute_batch("COMMIT")).await?;
        self.committed = true;
        Ok(())
    }
}

impl Deref for Transaction<'_> {
    type Target = Connection;

    fn deref(&self) -> &Self::Target {
        self.db
    }
}

impl DerefMut for Transaction<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.db
    }
}

impl Drop for Transaction<'_> {
    fn drop(&mut self) {
        if !self.committed {
            let _ = self.db.writer.send(Message::Rollback);
        }
    }
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
    Vec<u8> => Blob, std::convert::identity;
}

impl From<u64> for Value {
    fn from(value: u64) -> Self {
        Self::Unsigned(value)
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

pub(crate) async fn exec<const N: usize>(
    db: &mut Connection,
    statement: &str,
    args: [Value; N],
) -> Result<usize, Error> {
    let statement = statement.to_owned();
    db.call(move |db| {
        db.prepare_cached(&statement)?
            .execute(params_from_iter(args))
    })
    .await
}

pub(crate) async fn rows<const N: usize>(
    db: &mut Connection,
    query: &str,
    args: [Value; N],
) -> Result<impl Iterator<Item = Row> + use<N>, Error> {
    let query = query.to_owned();
    let rows = db
        .call(move |db| {
            let mut statement = db.prepare_cached(&query)?;
            let columns = statement.column_count();
            statement
                .query_map(params_from_iter(args), |row| {
                    (0..columns)
                        .map(|column| row.get(column))
                        .collect::<rusqlite::Result<Vec<_>>>()
                        .map(Row)
                })?
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .await?;
    Ok(rows.into_iter())
}

pub(crate) async fn row<const N: usize>(
    db: &mut Connection,
    query: &str,
    args: [Value; N],
) -> Result<Option<Row>, Error> {
    Ok(rows(db, query, args).await?.next())
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

    pub(crate) fn bytes(&mut self, col: usize) -> Result<Vec<u8>, Error> {
        match self.take(col) {
            SqlValue::Blob(value) => Ok(value),
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

    pub(crate) fn flag(&self, col: usize) -> Result<bool, Error> {
        Ok(self.i64(col)? != 0)
    }
}

fn invalid_column(col: usize) -> Error {
    Error::corrupt(format_args!("unexpected SQLite value in column {col}"))
}
