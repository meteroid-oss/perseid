use anyhow::{Result, anyhow};
use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use crypto_box::{PublicKey, aead::OsRng};
use serde_json::json;

use super::api::GitHub;

/// `value` in a libsodium sealed box for the base64 Curve25519 key, as GitHub expects secrets.
pub fn seal(public_key: &str, value: &[u8]) -> Result<String> {
    let key: [u8; 32] = BASE64
        .decode(public_key)?
        .try_into()
        .map_err(|_| anyhow!("GitHub sent a secrets key that isn't 32 bytes"))?;
    let sealed = PublicKey::from(key)
        .seal(&mut OsRng, value)
        .map_err(|e| anyhow!("encrypting a secret: {e}"))?;
    Ok(BASE64.encode(sealed))
}

pub fn set_secret(api: &GitHub, repo: &str, name: &str, value: &str) -> Result<()> {
    let key = api.get(&format!("/repos/{repo}/actions/secrets/public-key"))?;
    let (Some(id), Some(public)) = (key["key_id"].as_str(), key["key"].as_str()) else {
        return Err(anyhow!("GitHub sent no secrets key for {repo}"));
    };
    api.put(
        &format!("/repos/{repo}/actions/secrets/{name}"),
        json!({ "encrypted_value": seal(public, value.as_bytes())?, "key_id": id }),
    )?;
    Ok(())
}

pub fn has_secret(api: &GitHub, repo: &str, name: &str) -> Result<bool> {
    Ok(api
        .find(&format!("/repos/{repo}/actions/secrets/{name}"))?
        .is_some())
}

pub fn variable(api: &GitHub, repo: &str, name: &str) -> Result<Option<String>> {
    Ok(api
        .find(&format!("/repos/{repo}/actions/variables/{name}"))?
        .and_then(|v| v["value"].as_str().map(str::to_owned)))
}

/// Sets a repository variable, telling whether it changed.
pub fn set_variable(api: &GitHub, repo: &str, name: &str, value: &str) -> Result<bool> {
    let body = json!({ "name": name, "value": value });
    match variable(api, repo, name)? {
        Some(current) if current == value => return Ok(false),
        Some(_) => api.patch(&format!("/repos/{repo}/actions/variables/{name}"), body)?,
        None => api.post(&format!("/repos/{repo}/actions/variables"), body)?,
    };
    Ok(true)
}

#[cfg(test)]
mod tests {
    use crypto_box::SecretKey;

    use super::*;

    #[test]
    fn sealed_secrets_open_with_the_repository_key() {
        let secret = SecretKey::generate(&mut OsRng);
        let public = BASE64.encode(secret.public_key().as_bytes());
        let sealed = BASE64
            .decode(seal(&public, b"-----BEGIN RSA").unwrap())
            .unwrap();
        assert_eq!(secret.unseal(&sealed).unwrap(), b"-----BEGIN RSA");
        assert!(seal("c2hvcnQ=", b"x").is_err());
    }
}
