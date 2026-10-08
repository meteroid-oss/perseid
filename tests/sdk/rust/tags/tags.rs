//! Runs against the SDK generated from tests/sdk/rust/tags/openapi.yaml (see tests/sdk/run.sh):
//! union variants sharing a tag.
use serde_json::{json, Value};
use tags::models::{AssistantTurn, Conversation, SimpleTurn, Turn, UserTurn};

fn turn(value: Value) -> Turn {
    serde_json::from_value(value).unwrap()
}

#[test]
fn variants_sharing_a_tag_are_sent_with_it() {
    let turns: [Turn; 3] = [
        SimpleTurn::new("hi").into(),
        UserTurn::new(vec!["a".into()]).into(),
        AssistantTurn::new("m1", vec![]).into(),
    ];
    for turn in &turns {
        assert_eq!(serde_json::to_value(turn).unwrap()["type"], "message");
        assert_eq!(turn.tag(), Some("message"));
    }
}

#[test]
fn a_shared_tag_decodes_as_the_first_variant_the_data_fits() {
    assert!(matches!(turn(json!({"type": "message", "content": "hi"})), Turn::Message(_)));
    let user = turn(json!({"type": "message", "role": "user", "parts": ["a"]}));
    assert!(matches!(user, Turn::UserTurn(ref t) if t.parts == ["a"]));
    let assistant = turn(json!({"type": "message", "id": "m1", "parts": []}));
    assert!(matches!(assistant, Turn::AssistantTurn(ref t) if t.id == "m1"));
    let error = serde_json::from_value::<Turn>(json!({"type": "message"})).unwrap_err();
    assert!(error.to_string().contains("content"), "{error}");
}

#[test]
fn shared_tags_round_trip() {
    let value = json!({"turns": [
        {"type": "message", "content": "hi"},
        {"type": "message", "role": "user", "parts": ["a"]},
        {"type": "message", "id": "m1", "parts": []},
        {"type": "tool", "output": "42"},
    ]});
    let conversation: Conversation = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(serde_json::to_value(&conversation).unwrap(), value);
}
