    pub fn comment_retired(&mut self, id: &str) -> Result<bool> {
        Ok(self
            .client
            .query_one(
                "SELECT EXISTS (SELECT 1 FROM tl3_events WHERE event_key = $1)",
                &[&format!("retire-comment:{id}")],
            )?
            .get(0))
    }
