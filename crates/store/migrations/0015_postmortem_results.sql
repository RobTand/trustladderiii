-- #403 slice B: the MLflow assessments a postmortem's judges logged on the
-- node's traces, so the console can link each grade to its evidence.
ALTER TABLE tl3_postmortems ADD COLUMN assessment_ids jsonb;

-- A run that fails for a reason worth retrying makes its postmortem owed
-- again, at most twice, behind postmortems owed since (#486).
ALTER TABLE tl3_postmortems ADD COLUMN attempts integer NOT NULL DEFAULT 0
    CHECK (attempts BETWEEN 0 AND 2);
