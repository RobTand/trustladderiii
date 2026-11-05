//! #821: parser round-trips; full six-kind parity runs in controller::pb_evidence,
//! where the unchanged Python bounding and I/O shell can also execute.
use proptest::prelude::*;
use serde_json::{Value, json};
use tl3_pb_evidence::{Request, Response, evaluate, python_json};

proptest! {
    /// #821: arbitrary JSON byte inputs return a typed result, without panicking.
    #[test]
    fn byte_parser_is_total(raw in proptest::collection::vec(any::<u8>(), 0..4097), code in -128i64..128, private in any::<bool>(), detach in any::<bool>()) {
        let result = tl3_pb_evidence::transport::record(private,code,&raw,detach,false);
        let response = match result { Ok(v) => Response::Return(v), Err(e) => Response::Raise(e) };
        let encoded = serde_json::to_vec(&response).unwrap();
        let _: Response = serde_json::from_slice(&encoded).unwrap();
    }

    /// #821 / Q55: Unicode, integers, arrays and bools retain their JSON distinctions.
    #[test]
    fn serializer_roundtrip(s in ".{0,200}", n in any::<i64>(), b in any::<bool>(), ascii in any::<bool>(), compact in any::<bool>()) {
        let value = json!({"text":s,"number":n,"boolean":b,"array":[null,1,1.0,-0.0]});
        let text = python_json::dumps(&value,ascii,compact);
        let decoded: Value = serde_json::from_str(&text).unwrap();
        prop_assert_eq!(&value,&decoded);
        prop_assert_eq!(text,python_json::dumps(&decoded,ascii,compact));
    }

    /// #821: strict request parsing preserves fields across the serde boundary.
    #[test]
    fn request_roundtrip(base in ".{0,100}", selectors in proptest::collection::vec(".{0,80}",0..12)) {
        let request = Request::Identity { base_sha: json!(base), selectors: json!(selectors), forcing: serde_json::from_value(json!({"symptom":"fixture", "environment":{},"patches":[]})).unwrap(),declared_symptom:Value::Null };
        let encoded=serde_json::to_vec(&request).unwrap();
        let decoded: Request=serde_json::from_slice(&encoded).unwrap();
        prop_assert_eq!(evaluate(request).unwrap(),evaluate(decoded).unwrap());
    }
}
