//! State-table "Counter headroom": every write made while translating an input
//! participates in its transaction, including helpers that normally commit alone.
use crate::{DatabaseError, Result};
use postgres::{Client, GenericClient, Row, Transaction, types::ToSql};

type Params<'a> = &'a [&'a (dyn ToSql + Sync)];

/// Query interface shared by ordinary Postgres transactions and input savepoints.
/// The reader API continues to expose the driver's transaction type.
pub trait Sql {
    fn query(
        &mut self,
        sql: &str,
        params: Params<'_>,
    ) -> std::result::Result<Vec<Row>, DatabaseError>;
    fn query_one(
        &mut self,
        sql: &str,
        params: Params<'_>,
    ) -> std::result::Result<Row, DatabaseError>;
    fn query_opt(
        &mut self,
        sql: &str,
        params: Params<'_>,
    ) -> std::result::Result<Option<Row>, DatabaseError>;
    fn execute(&mut self, sql: &str, params: Params<'_>)
    -> std::result::Result<u64, DatabaseError>;
    fn batch_execute(&mut self, sql: &str) -> std::result::Result<(), DatabaseError>;
}

impl<T: GenericClient> Sql for T {
    fn query(&mut self, sql: &str, p: Params<'_>) -> std::result::Result<Vec<Row>, DatabaseError> {
        GenericClient::query(self, sql, p)
    }
    fn query_one(&mut self, sql: &str, p: Params<'_>) -> std::result::Result<Row, DatabaseError> {
        GenericClient::query_one(self, sql, p)
    }
    fn query_opt(
        &mut self,
        sql: &str,
        p: Params<'_>,
    ) -> std::result::Result<Option<Row>, DatabaseError> {
        GenericClient::query_opt(self, sql, p)
    }
    fn execute(&mut self, sql: &str, p: Params<'_>) -> std::result::Result<u64, DatabaseError> {
        GenericClient::execute(self, sql, p)
    }
    fn batch_execute(&mut self, sql: &str) -> std::result::Result<(), DatabaseError> {
        GenericClient::batch_execute(self, sql)
    }
}

pub(crate) enum Operation<'a> {
    Standalone(Option<Transaction<'a>>),
    Input {
        client: &'a mut Client,
        finished: bool,
    },
}

impl Sql for Operation<'_> {
    fn query(&mut self, sql: &str, p: Params<'_>) -> std::result::Result<Vec<Row>, DatabaseError> {
        match self {
            Self::Standalone(t) => t.as_mut().expect("open operation").query(sql, p),
            Self::Input { client, .. } => client.query(sql, p),
        }
    }
    fn query_one(&mut self, sql: &str, p: Params<'_>) -> std::result::Result<Row, DatabaseError> {
        match self {
            Self::Standalone(t) => t.as_mut().expect("open operation").query_one(sql, p),
            Self::Input { client, .. } => client.query_one(sql, p),
        }
    }
    fn query_opt(
        &mut self,
        sql: &str,
        p: Params<'_>,
    ) -> std::result::Result<Option<Row>, DatabaseError> {
        match self {
            Self::Standalone(t) => t.as_mut().expect("open operation").query_opt(sql, p),
            Self::Input { client, .. } => client.query_opt(sql, p),
        }
    }
    fn execute(&mut self, sql: &str, p: Params<'_>) -> std::result::Result<u64, DatabaseError> {
        match self {
            Self::Standalone(t) => t.as_mut().expect("open operation").execute(sql, p),
            Self::Input { client, .. } => client.execute(sql, p),
        }
    }
    fn batch_execute(&mut self, sql: &str) -> std::result::Result<(), DatabaseError> {
        match self {
            Self::Standalone(t) => t.as_mut().expect("open operation").batch_execute(sql),
            Self::Input { client, .. } => client.batch_execute(sql),
        }
    }
}

impl Operation<'_> {
    pub(crate) fn commit(mut self) -> Result<()> {
        match &mut self {
            Self::Standalone(tx) => {
                tx.take().expect("open operation").commit()?;
            }
            Self::Input { client, finished } => {
                client.batch_execute("RELEASE SAVEPOINT tl3_operation")?;
                *finished = true;
            }
        }
        Ok(())
    }
}

impl Drop for Operation<'_> {
    fn drop(&mut self) {
        if let Self::Input {
            client,
            finished: false,
        } = self
        {
            let _ = client.batch_execute(
                "ROLLBACK TO SAVEPOINT tl3_operation; RELEASE SAVEPOINT tl3_operation",
            );
        }
        // The driver rolls an uncommitted standalone transaction back on drop.
    }
}

pub(crate) fn operation(client: &mut Client, input: bool) -> Result<Operation<'_>> {
    if input {
        client.batch_execute("SAVEPOINT tl3_operation")?;
        Ok(Operation::Input {
            client,
            finished: false,
        })
    } else {
        Ok(Operation::Standalone(Some(client.transaction()?)))
    }
}
