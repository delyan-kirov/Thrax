use super::*;

#[test]
fn round_trips_a_message() {
    let text = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"a":[1,2.5,true,null,"x\ny"]}}"#;
    let v = Json::parse(text).unwrap();
    assert_eq!(v.get("id").as_i64(), Some(1));
    assert_eq!(v.get("params").get("a").as_arr().len(), 5);
    assert_eq!(v.to_string(), text);
}

#[test]
fn missing_members_read_as_null() {
    let v = Json::parse("{}").unwrap();
    assert!(v.get("x").get("y").is_null());
}

#[test]
fn decodes_escapes_and_surrogate_pairs() {
    let v = Json::parse(r#""é😀\t""#).unwrap();
    assert_eq!(v.as_str(), Some("é😀\t"));
}

#[test]
fn escapes_control_characters_on_write() {
    assert_eq!(Json::str("a\u{1}\"").to_string(), r#""a\u0001\"""#);
}

#[test]
fn rejects_trailing_garbage() {
    assert!(Json::parse("1 2").is_err());
    assert!(Json::parse("[1,").is_err());
}

#[test]
fn set_replaces_or_appends() {
    let mut v = Json::obj([("a", Json::int(1))]);
    v.set("a", Json::int(2));
    v.set("b", Json::Null);
    assert_eq!(v.to_string(), r#"{"a":2,"b":null}"#);
}
