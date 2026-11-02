-- #403 slice B: the MLflow assessments a postmortem's judges logged on the
-- node's traces, so the console can link each grade to its evidence.
ALTER TABLE tl3_postmortems ADD COLUMN assessment_ids jsonb;
