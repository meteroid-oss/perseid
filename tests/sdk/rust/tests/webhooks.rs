use http1::{HeaderMap, HeaderName, HeaderValue};
use petstore::webhooks::{Webhook, WebhookError};
use std::time::{SystemTime, UNIX_EPOCH};

const SECRET: &str = "whsec_MfKQ9r8GKYqrTwjUPD8ILPZIo2LaLaSw";
const PAYLOAD: &[u8] = br#"{"test": 2432232314}"#;

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

fn headers(prefix: &str, signature: &str, timestamp: i64) -> HeaderMap {
    let mut map = HeaderMap::new();
    for (name, value) in [
        ("id", "msg_1".to_owned()),
        ("signature", signature.to_owned()),
        ("timestamp", timestamp.to_string()),
    ] {
        map.insert(
            HeaderName::try_from(format!("{prefix}-{name}")).unwrap(),
            HeaderValue::from_str(&value).unwrap(),
        );
    }
    map
}

#[test]
fn signs_like_the_standard_webhooks_test_vector() {
    let webhook = Webhook::new(SECRET).unwrap();
    assert_eq!(
        webhook.sign("msg_p5jXN8AQM9LWM0D4loKWxJek", 1614265330, PAYLOAD),
        "v1,g0hM9SsE+OTPJTGt/tmIKtSyZlE3uFJELVlNIOLJ1OE="
    );
}

#[test]
fn verifies_both_header_families() {
    let webhook = Webhook::new(SECRET).unwrap();
    let signature = webhook.sign("msg_1", now(), PAYLOAD);
    for prefix in ["webhook", "svix"] {
        webhook
            .verify(PAYLOAD, &headers(prefix, &signature, now()))
            .unwrap();
    }
}

#[test]
fn rejects_bad_input() {
    let webhook = Webhook::new(SECRET).unwrap();
    let good = webhook.sign("msg_1", now(), PAYLOAD);
    let verify = |payload: &[u8], headers: &HeaderMap| webhook.verify(payload, headers);

    assert_eq!(
        verify(b"tampered", &headers("webhook", &good, now())),
        Err(WebhookError::NoMatchingSignature)
    );
    assert_eq!(
        verify(PAYLOAD, &HeaderMap::new()),
        Err(WebhookError::MissingHeaders)
    );
    let old = now() - 3600;
    let signature = webhook.sign("msg_1", old, PAYLOAD);
    assert_eq!(
        verify(PAYLOAD, &headers("webhook", &signature, old)),
        Err(WebhookError::TimestampTooOld)
    );
    let future = now() + 3600;
    let signature = webhook.sign("msg_1", future, PAYLOAD);
    assert_eq!(
        verify(PAYLOAD, &headers("webhook", &signature, future)),
        Err(WebhookError::TimestampTooNew)
    );
}

#[test]
fn standard_headers_win_and_rotated_signatures_are_accepted() {
    let webhook = Webhook::new(SECRET).unwrap();
    let good = webhook.sign("msg_1", now(), PAYLOAD);

    let mut mixed = headers("svix", &good, now());
    mixed.extend(headers("webhook", "v1,AAAA", now()));
    assert_eq!(
        webhook.verify(PAYLOAD, &mixed),
        Err(WebhookError::NoMatchingSignature)
    );

    let rotated = format!("v1,AAAA v2,{good} {good}");
    webhook
        .verify(PAYLOAD, &headers("webhook", &rotated, now()))
        .unwrap();
}

#[test]
fn rejects_unusable_secrets() {
    assert!(matches!(
        Webhook::new("whsec_!!!"),
        Err(WebhookError::InvalidSecret)
    ));
    assert!(matches!(
        Webhook::new("whsec_"),
        Err(WebhookError::InvalidSecret)
    ));
    assert!(Webhook::from_bytes(Vec::new()).is_err());
}
