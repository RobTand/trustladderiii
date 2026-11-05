use super::{call, json, with};
type Result<T> = std::result::Result<T, tl3_store::Error>;
pub(crate) struct Store {
    client: Client,
}
impl Store {
    pub(super) fn new() -> Self {
        Self { client: Client }
    }
    pub fn comment_retired(&mut self, id: &str) -> Result<bool> {
        Ok(self
            .client
            .query_one(
                "SELECT EXISTS (SELECT 1 FROM tl3_events WHERE event_key = $1)",
                &[&format!("retire-comment:{id}")],
            )?
            .get(0))
    }
}
struct Client;
struct QueryRow(bool);
impl QueryRow {
    fn get(&self, _: usize) -> bool {
        self.0
    }
}
impl Client {
    fn query_one(
        &mut self,
        sql: &str,
        parameters: &[&String],
    ) -> std::result::Result<QueryRow, tl3_store::DatabaseError> {
        let reply = with(|r| {
            let v = r.tape.get(r.run.reads).copied().unwrap_or(0) % 4;
            r.run.reads += 1;
            v
        });
        call(
            "store.comment_retired",
            json!({"id":parameters[0].strip_prefix("retire-comment:").unwrap(),"sql":sql,"parameters":parameters,"reply":reply}),
        );
        match reply {
            0 => Ok(QueryRow(false)),
            1 => Ok(QueryRow(true)),
            _ => Err(tl3_store::DatabaseError::__private_api_timeout()),
        }
    }
}
