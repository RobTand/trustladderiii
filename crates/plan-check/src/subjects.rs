mod decision;

pub fn subject_covers(chosen: &str, subject: &str) -> bool {
    decision::covers(chosen.as_bytes(), subject.as_bytes())
}

pub fn overlaps(left: &str, right: &str) -> bool {
    subject_covers(left, right) || subject_covers(right, left)
}
