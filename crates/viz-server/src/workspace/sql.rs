use anyhow::anyhow;
use toasty::Executor;
pub(crate) use toasty_core::stmt::Value;
use toasty_core::{schema::db::Type, stmt::ValueRecord};

use crate::wire::RpcError;

pub(crate) async fn exec<const N: usize>(
    db: &mut dyn Executor,
    statement: &str,
    args: [Value; N],
) -> Result<u64, RpcError> {
    let statement = args
        .into_iter()
        .fold(toasty::sql::statement(statement), |sql, arg| {
            if arg.is_null() {
                sql.bind_typed(arg, Type::Text)
            } else {
                sql.bind(arg)
            }
        });
    statement.exec(db).await.map_err(RpcError::internal)
}

pub(crate) async fn rows<const N: usize>(
    db: &mut dyn Executor,
    query: &str,
    args: [Value; N],
) -> Result<impl Iterator<Item = Row> + use<N>, RpcError> {
    let query = args
        .into_iter()
        .fold(toasty::sql::query(query), |sql, arg| {
            if arg.is_null() {
                sql.bind_typed(arg, Type::Text)
            } else {
                sql.bind(arg)
            }
        });
    let rows = query.exec(db).await.map_err(RpcError::internal)?;
    Ok(rows.into_iter().map(|value| Row(value.into_record())))
}

pub(crate) async fn row<const N: usize>(
    db: &mut dyn Executor,
    query: &str,
    args: [Value; N],
) -> Result<Option<Row>, RpcError> {
    Ok(rows(db, query, args).await?.next())
}

#[derive(Debug)]
pub(crate) struct Row(ValueRecord);

impl Row {
    fn take(&mut self, col: usize) -> Value {
        self.0[col].take()
    }

    pub(crate) fn str(&mut self, col: usize) -> Result<String, RpcError> {
        self.opt_str(col)?.ok_or_else(|| invalid_column(col))
    }

    pub(crate) fn opt_str(&mut self, col: usize) -> Result<Option<String>, RpcError> {
        match self.take(col) {
            Value::String(value) => Ok(Some(value)),
            Value::Null => Ok(None),
            _ => Err(invalid_column(col)),
        }
    }

    pub(crate) fn bytes(&mut self, col: usize) -> Result<Vec<u8>, RpcError> {
        match self.take(col) {
            Value::Bytes(value) => Ok(value),
            _ => Err(invalid_column(col)),
        }
    }

    pub(crate) fn i64(&self, col: usize) -> Result<i64, RpcError> {
        self.0[col].to_i64().ok_or_else(|| invalid_column(col))
    }

    pub(crate) fn u64(&self, col: usize) -> Result<u64, RpcError> {
        self.opt_u64(col)?.ok_or_else(|| invalid_column(col))
    }

    pub(crate) fn opt_u64(&self, col: usize) -> Result<Option<u64>, RpcError> {
        let value = &self.0[col];
        if value.is_null() {
            Ok(None)
        } else {
            value.to_u64().map(Some).ok_or_else(|| invalid_column(col))
        }
    }

    pub(crate) fn flag(&self, col: usize) -> Result<bool, RpcError> {
        Ok(self.i64(col)? != 0)
    }
}

fn invalid_column(col: usize) -> RpcError {
    RpcError::internal(anyhow!("unexpected SQLite value in column {col}"))
}
