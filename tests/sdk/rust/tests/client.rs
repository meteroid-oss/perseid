//! tests/sdk/run.sh generates this SDK without a base URL.
use petstore::{api::Petstore, error::Error};

#[test]
fn a_base_url_is_required() {
    std::env::remove_var("PETSTORE_BASE_URL");
    let error = Petstore::builder().token("t").build().unwrap_err();
    assert!(matches!(error, Error::Request(_)), "{error:?}");
    let message = error.to_string();
    assert!(message.contains("base_url()") && message.contains("PETSTORE_BASE_URL"), "{message}");
    assert!(Petstore::from_env().is_err());
    assert!(Petstore::new("t").is_err());

    let explicit = Petstore::builder().base_url("https://pets.example.com").build().unwrap();
    assert!(format!("{explicit:?}").contains("https://pets.example.com"));
    std::env::set_var("PETSTORE_BASE_URL", "https://env.example.com");
    let from_env = Petstore::from_env().unwrap();
    let builder = Petstore::builder().build();
    std::env::remove_var("PETSTORE_BASE_URL");
    assert!(format!("{from_env:?}").contains("https://env.example.com"));
    assert!(builder.is_err(), "the builder reads no environment variable");
}
