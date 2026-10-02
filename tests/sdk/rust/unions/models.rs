//! Runs against the SDK generated from tests/fixtures/edge-unions.yaml (see tests/sdk/run.sh):
//! decoding and encoding of unions sharing a JSON type, best-match object unions, open enums and
//! models with a field no SDK can type.
use serde_json::{json, Value};
use unions_sdk::models::{
    AllowedTools, Completion, CompletionCreatedAt, CompletionPrompt, CompletionStop,
    CreateCompletionRequest, CreateCompletionRequestPrompt, Include, ImageRef, ModelIds,
    ModelIdsWithRef, ToolChoice, Untypable, Voice, VoiceWithOpen,
};

fn completion(extra: Value) -> Completion {
    let mut value = json!({
        "id": "c",
        "model": "alpha-1",
        "created_at": "2024-01-02T03:04:05Z",
        "choices": [],
    });
    value.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
    serde_json::from_value(value).unwrap()
}

#[test]
fn prompts_pick_the_variant_of_their_json_value() {
    let c = completion(json!({"prompt": "hi"}));
    assert!(matches!(c.prompt, Some(CompletionPrompt::String(ref s)) if s == "hi"));
    let c = completion(json!({"prompt": ["a", "b"]}));
    assert!(matches!(c.prompt, Some(CompletionPrompt::ArrayOfStrings(ref v)) if v == &["a", "b"]));
    let c = completion(json!({"prompt": [1, 2]}));
    assert!(matches!(c.prompt, Some(CompletionPrompt::ArrayOfIntegers(ref v)) if v == &[1, 2]));
    let c = completion(json!({"prompt": [[1], [2, 3]]}));
    assert!(
        matches!(c.prompt, Some(CompletionPrompt::ArrayOfIntegerArrays(ref v)) if v == &[vec![1], vec![2, 3]])
    );
}

#[test]
fn an_empty_array_is_the_first_array_variant() {
    let c = completion(json!({"prompt": []}));
    assert!(matches!(c.prompt, Some(CompletionPrompt::ArrayOfStrings(ref v)) if v.is_empty()));
}

#[test]
fn values_no_variant_decodes_are_kept_as_received() {
    // The first item selects `array_of_strings`, which then fails to decode.
    let c = completion(json!({"prompt": ["a", 1]}));
    assert!(matches!(c.prompt, Some(CompletionPrompt::Unknown(ref v)) if *v == json!(["a", 1])));
    assert_eq!(serde_json::to_value(&c.prompt).unwrap(), json!(["a", 1]));
}

#[test]
fn prompts_encode_as_the_variant_they_hold() {
    let cases = [
        (CreateCompletionRequestPrompt::from("hi"), json!("hi")),
        (CreateCompletionRequestPrompt::from(vec!["a".to_owned()]), json!(["a"])),
        (CreateCompletionRequestPrompt::from(vec![1_i64, 2]), json!([1, 2])),
        (CreateCompletionRequestPrompt::from(vec![vec![1_i64], vec![2]]), json!([[1], [2]])),
    ];
    for (prompt, wire) in cases {
        let request = CreateCompletionRequest::new(ModelIds::from("alpha-1"), prompt.clone());
        let body = serde_json::to_value(&request).unwrap();
        assert_eq!(body["prompt"], wire);
        let back: CreateCompletionRequest = serde_json::from_value(body).unwrap();
        assert_eq!(back.prompt, prompt);
    }
}

#[test]
fn date_times_fall_back_to_strings() {
    let c = completion(json!({"created_at": "2024-01-02T03:04:05Z"}));
    assert!(matches!(c.created_at, CompletionCreatedAt::DateTime(_)));
    let c = completion(json!({"created_at": "yesterday"}));
    assert!(matches!(c.created_at, CompletionCreatedAt::String(ref s) if s == "yesterday"));
    assert_eq!(serde_json::to_value(&c.created_at).unwrap(), json!("yesterday"));
}

#[test]
fn type_arrays_become_unions_or_numbers() {
    let c = completion(json!({"stop": 5, "ratio": 1, "score": 2.5, "limit": null}));
    assert!(matches!(c.stop, Some(CompletionStop::Integer(5))));
    assert_eq!(c.ratio, Some(1.0));
    assert_eq!(c.score, Some(2.5));
    assert_eq!(c.limit, None);
    let c = completion(json!({"stop": "x"}));
    assert!(matches!(c.stop, Some(CompletionStop::String(ref s)) if s == "x"));
}

#[test]
fn object_unions_pick_the_best_matching_variant() {
    let cases = [
        json!("auto"),
        json!({"mode": "auto", "tools": [{"name": "f"}]}),
        json!({"type": "web_search"}),
        json!({"name": "f", "arguments": "{}"}),
    ];
    let picked: Vec<ToolChoice> = cases
        .iter()
        .map(|case| serde_json::from_value(case.clone()).unwrap())
        .collect();
    assert!(matches!(&picked[0], ToolChoice::String(s) if s == "auto"));
    assert!(matches!(&picked[1], ToolChoice::AllowedTools(t) if t.mode.as_str() == "auto"));
    assert!(matches!(&picked[2], ToolChoice::HostedTool(_)));
    assert!(matches!(&picked[3], ToolChoice::FunctionTool(f) if f.name == "f"));
    for (case, choice) in cases.iter().zip(&picked) {
        assert_eq!(&serde_json::to_value(choice).unwrap(), case);
    }
}

#[test]
fn objects_no_variant_matches_are_kept() {
    let choice: ToolChoice = serde_json::from_value(json!({"unrelated": true})).unwrap();
    assert!(matches!(choice, ToolChoice::Unknown(ref v) if *v == json!({"unrelated": true})));
    let tools: AllowedTools =
        serde_json::from_value(json!({"mode": "required", "tools": []})).unwrap();
    assert_eq!(tools.tools.len(), 0);
}

#[test]
fn open_enums_keep_unknown_values() {
    for (wire, known) in [("alpha-1", true), ("beta-1", true), ("gamma", false)] {
        let model: ModelIds = serde_json::from_value(json!(wire)).unwrap();
        assert_eq!(model.as_str(), wire);
        assert_eq!(!matches!(model, ModelIds::Unknown(_)), known, "{wire}");
        assert_eq!(serde_json::to_value(&model).unwrap(), json!(wire));
    }
    // `string | $ref enum`: the values of the referenced enum are known too.
    assert!(!matches!(ModelIdsWithRef::from("chat-large"), ModelIdsWithRef::Unknown(_)));
    assert!(matches!(ModelIdsWithRef::from("other"), ModelIdsWithRef::Unknown(_)));
    assert!(!matches!(ModelIdsWithRef::from("chat-small"), ModelIdsWithRef::Unknown(_)));
}

#[test]
fn enums_of_consts_and_enums_merge_their_values() {
    for value in ["logprobs", "usage", "sources"] {
        assert!(!matches!(Include::from(value), Include::Unknown(_)), "{value}");
    }
    assert!(matches!(Include::from("new"), Include::Unknown(_)));
    for value in ["alloy", "ash", "coral", "sage"] {
        assert!(!matches!(Voice::from(value), Voice::Unknown(_)), "{value}");
        assert!(!matches!(VoiceWithOpen::from(value), VoiceWithOpen::Unknown(_)), "{value}");
    }
    assert!(matches!(Voice::from("verse"), Voice::Unknown(_)));
    assert_eq!(VoiceWithOpen::from("verse").to_string(), "verse");
}

#[test]
fn open_enum_fields_decode_in_models() {
    let c = completion(json!({
        "model": "never-seen",
        "voice_open": "sage",
        "include": ["usage", "future"],
        "fallback_model": "chat-small",
    }));
    assert_eq!(c.model.as_str(), "never-seen");
    assert_eq!(c.voice_open.unwrap().as_str(), "sage");
    let include = c.include.unwrap();
    assert_eq!(include[0].as_str(), "usage");
    assert!(matches!(include[1], Include::Unknown(_)));
    assert_eq!(c.fallback_model.unwrap().as_str(), "chat-small");
}

#[test]
fn an_untypable_field_does_not_untype_its_model() {
    let u: Untypable = serde_json::from_value(json!({"name": "n", "mixed": "auto", "count": 3}))
        .unwrap();
    assert_eq!(u.name, "n");
    assert_eq!(u.mixed, Some(json!("auto")));
    assert_eq!(u.count, Some(3));
}

#[test]
fn required_only_alternatives_leave_a_struct() {
    let image: ImageRef = serde_json::from_value(json!({"file_id": "f"})).unwrap();
    assert_eq!(image.file_id.as_deref(), Some("f"));
    assert_eq!(image.image_url, None);
    assert_eq!(serde_json::to_value(&image).unwrap(), json!({"file_id": "f"}));
}
