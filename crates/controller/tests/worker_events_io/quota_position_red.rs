/// Q53/#759, state-table cdacce8f :277,787. Red reproduction of the base's
/// byte-equality quota-finality hole; a behavior fix is outside this extraction.
#[test]
fn quota_must_be_at_the_last_position_red() {
    let mut bytes = line(3, false);
    bytes.extend(line(0, false));
    bytes.extend(line(3, false));
    let d = delivery(false);
    assert!(reference::parse_reply(&bytes, &d, &Value::Null).is_err());
}

