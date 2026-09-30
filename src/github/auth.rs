use std::{
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use anyhow::{Result, bail};

use super::{
    Ui,
    api::{GitHub, check, post_form},
};

/// perseid's OAuth App, for the device flow. Public by design: device flow clients have no secret.
const CLIENT_ID: &str = "Ov23liBrNho4EP0xC7Ob";

/// A user token from the environment or the gh CLI.
pub fn stored_token() -> Option<(String, &'static str)> {
    for var in ["GH_TOKEN", "GITHUB_TOKEN"] {
        if let Some(token) = std::env::var(var).ok().filter(|t| !t.trim().is_empty()) {
            return Some((token.trim().to_owned(), var));
        }
    }
    gh_token().map(|token| (token, "gh"))
}

/// A user token, from the environment, the gh CLI, or the device flow. Never written anywhere.
pub fn token(ui: &Ui) -> Result<(String, &'static str)> {
    if let Some(found) = stored_token() {
        return Ok(found);
    }
    let client_id = match std::env::var("PERSEID_GITHUB_CLIENT_ID") {
        Ok(id) => id,
        Err(_) => CLIENT_ID.to_owned(),
    };
    if client_id.is_empty() {
        bail!(
            "no GitHub credentials: set GH_TOKEN to a token with the `repo` and `workflow` scopes, or install gh and run `gh auth login`"
        );
    }
    Ok((device_flow(&client_id, ui)?, "browser"))
}

fn gh_token() -> Option<String> {
    let output = Command::new("gh")
        .args(["auth", "token"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let token = String::from_utf8(output.stdout).ok()?.trim().to_owned();
    (output.status.success() && !token.is_empty()).then_some(token)
}

fn device_flow(client_id: &str, ui: &Ui) -> Result<String> {
    let code = post_form(
        "/login/device/code",
        &[("client_id", client_id), ("scope", "repo workflow")],
    )?;
    let (Some(device_code), Some(user_code), Some(uri)) = (
        code["device_code"].as_str(),
        code["user_code"].as_str(),
        code["verification_uri"].as_str(),
    ) else {
        bail!(
            "GitHub refused the device login: {}",
            code["error_description"].as_str().unwrap_or("no details")
        );
    };
    ui.say(&format!("Open {uri} and enter the code {user_code}"));
    ui.open(uri);
    let mut interval = code["interval"].as_u64().unwrap_or(5);
    let deadline = Instant::now() + Duration::from_secs(code["expires_in"].as_u64().unwrap_or(900));
    loop {
        std::thread::sleep(Duration::from_secs(interval));
        let reply = post_form(
            "/login/oauth/access_token",
            &[
                ("client_id", client_id),
                ("device_code", device_code),
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ],
        )?;
        if let Some(token) = reply["access_token"].as_str() {
            return Ok(token.to_owned());
        }
        match reply["error"].as_str() {
            Some("authorization_pending") => {}
            Some("slow_down") => interval = reply["interval"].as_u64().unwrap_or(interval + 5),
            Some("expired_token") => bail!("the login code expired, run the command again"),
            Some("access_denied") => bail!("the GitHub login was cancelled"),
            _ => bail!(
                "GitHub login failed: {}",
                reply["error_description"]
                    .as_str()
                    .or(reply["error"].as_str())
                    .unwrap_or("no details")
            ),
        }
        if Instant::now() > deadline {
            bail!("the login code expired, run the command again");
        }
    }
}

/// The login of the token's user, after checking a classic token may push workflow files.
pub fn whoami(api: &GitHub, source: &str) -> Result<String> {
    let reply = api.send("GET", "/user", None)?;
    if let Some(scopes) = &reply.scopes
        && !scopes.split(',').any(|s| s.trim() == "workflow")
    {
        let fix = match source {
            "gh" => "run `gh auth refresh -s workflow`",
            _ => "use a token with the `repo` and `workflow` scopes",
        };
        bail!("the GitHub token can't commit workflow files: {fix}");
    }
    let user = check("GET", "/user", reply)?;
    Ok(user["login"].as_str().unwrap_or_default().to_owned())
}
