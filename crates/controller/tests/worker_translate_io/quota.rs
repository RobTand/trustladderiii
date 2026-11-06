use super::*;
impl Controller {
    pub(crate) fn clear_model_quota(
        &self,
        snapshot: &Snapshot,
        model: Option<&str>,
        commit: &mut Commit,
    ) {
        if let Some(row) = snapshot
            .rows
            .iter()
            .find(|row| row.table == "model_quotas" && Some(row.id.as_str()) == model)
        {
            commit.rows.push(RowChange::Delete {
                table: row.table.clone(),
                id: row.id.clone(),
            });
        }
    }
}
