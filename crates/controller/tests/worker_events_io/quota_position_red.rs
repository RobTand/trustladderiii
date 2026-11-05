/// Q53 quota ending; positional finality comes from parse_reply's terminal
/// quota rule. The frozen references expose the bug; production rejects it.
#[test]
fn quota_must_be_at_the_last_position_red() {
    let mut bytes = line(3, false);
    bytes.extend(line(0, false));
    bytes.extend(line(3, false));
    let d = delivery(false);
    assert!(reference::parse_reply(&bytes, &d, &Value::Null).is_ok());
    assert!(pre_fix::parse_reply(&bytes, &d, &Value::Null).is_ok());
    let expected = outcome(Err(Error::Invalid("event after quota ending")));
    assert_eq!(outcome(current::parse_reply(&bytes, &d, &Value::Null)), expected);
    assert_eq!(outcome(super::parse_reply(&bytes, &d, &Value::Null)), expected);
}
