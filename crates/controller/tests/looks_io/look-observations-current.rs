    pub(crate) fn look_observations(
        &mut self,
        delivery: &Delivery,
        completion: Completion,
    ) -> Result<()> {
        let snapshot = self.store.load()?;
        let mut commit = Commit::new(
            format!(
                "look-observations:{}:{}",
                delivery.key,
                snapshot.revision.get()
            ),
            json!(&completion),
            &snapshot,
        );
        self.release_terminal_model_probes(&snapshot, &mut commit, None)?;
        self.retire_completed_model_attempts(&snapshot, &mut commit)?;
        if let Some(class) = completion.failure_class.as_deref() {
            self.failure_allowance(&snapshot, &delivery.key, class, &mut commit)?;
        }
        for (index, observation) in completion.observations.iter().enumerate() {
            self.queue_observation(&snapshot, &mut commit, &delivery.key, index, observation)?;
            if let Some(source) = crate::observation_upstream(&snapshot, observation)? {
                self.retry_held_effects(source);
            }
        }
        if !commit.rows.is_empty() {
            self.store.commit(&commit)?;
        }
        Ok(())
    }

