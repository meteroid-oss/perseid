//! Runs against the SDK generated from tests/fixtures/edge-unions.yaml (see tests/sdk/run.sh):
//! a union property its variants declare too.
use serde_json::json;
use unions_sdk::models::{Approval, ApprovalSubmit};

#[test]
fn a_union_property_its_variants_declare_reaches_them() {
    let value = json!({"type": "browser_authentication", "action": "submit", "code": "123"});
    let approval: Approval = serde_json::from_value(value.clone()).unwrap();
    assert!(matches!(approval, Approval::Submit(ref s) if s.r#type == "browser_authentication"));
    assert_eq!(serde_json::to_value(&approval).unwrap(), value);
    let sent = serde_json::to_value(Approval::from(ApprovalSubmit::new("123"))).unwrap();
    assert_eq!(sent, value);
}
