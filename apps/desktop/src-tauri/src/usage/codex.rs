//! Read-only Codex credentials for usage. Never redeem its rotating refresh token.
use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::Deserialize;
use serde_json::Value;
use std::{
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

pub(super) struct Credentials {
    pub access_token: String,
    pub account_id: String,
    pub fedramp_account: bool,
}

#[derive(Deserialize)]
struct AuthFile {
    auth_mode: Option<String>,
    tokens: Option<Tokens>,
}

#[derive(Deserialize)]
struct Tokens {
    access_token: String,
    id_token: String,
    account_id: Option<String>,
}

fn claims(token: &str) -> Option<Value> {
    let parts: Vec<_> = token.split('.').collect();
    if parts.len() != 3 || parts.iter().any(|part| part.is_empty()) {
        return None;
    }
    serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[1]).ok()?).ok()
}

fn parse(
    json: &[u8],
    email: Option<&str>,
    account_id: Option<&str>,
    now: u64,
) -> Result<Credentials> {
    // Do not include serde's error: malformed credential values can contain secrets.
    let auth: AuthFile = serde_json::from_slice(json).map_err(|_| {
        anyhow::anyhow!("Could not read the local Codex login. Open Codex to sign in again.")
    })?;
    if auth
        .auth_mode
        .as_deref()
        .is_some_and(|mode| mode != "chatgpt")
    {
        bail!("Sign in to Codex with ChatGPT to read plan usage, or view usage in ChatGPT.");
    }
    let tokens = auth.tokens.context("No local Codex ChatGPT login. Sign in to Codex with the same ChatGPT account, then refresh, or view usage in ChatGPT.")?;
    let identity = claims(&tokens.id_token)
        .context("Could not identify the local Codex account. Sign in to Codex again.")?;
    let access = claims(&tokens.access_token)
        .context("The local Codex login is invalid. Sign in to Codex again.")?;
    let codex_account_id = tokens
        .account_id
        .filter(|id| !id.trim().is_empty())
        .or_else(|| {
            identity["https://api.openai.com/auth"]["chatgpt_account_id"]
                .as_str()
                .map(str::to_owned)
        })
        .filter(|id| !id.trim().is_empty())
        .context("The local Codex login has no account ID. Sign in to Codex again.")?;
    // Claims identify locally stored credentials, not authorization. The service
    // validates the unchanged bearer token. Never show another user's limits.
    let same_email = email
        .filter(|email| !email.trim().is_empty())
        .is_some_and(|email| {
            identity["email"]
                .as_str()
                .is_some_and(|other| email.eq_ignore_ascii_case(other))
        });
    let same_account = account_id.is_none_or(|id| id == codex_account_id);
    if !same_email || !same_account {
        bail!("The local Codex login belongs to a different or unverified account. Sign in to Codex with the same ChatGPT account as Lathe, or view usage in ChatGPT.");
    }
    if !access["exp"]
        .as_u64()
        .is_some_and(|expiry| expiry > now.saturating_add(60))
    {
        bail!("The local Codex login needs refreshing. Open Codex to refresh its login, then refresh here, or view usage in ChatGPT.");
    }
    Ok(Credentials {
        access_token: tokens.access_token,
        account_id: codex_account_id,
        fedramp_account: identity["https://api.openai.com/auth"]["chatgpt_account_is_fedramp"]
            .as_bool()
            .unwrap_or(false),
    })
}

fn codex_home() -> Result<PathBuf> {
    match std::env::var_os("CODEX_HOME").filter(|value| !value.is_empty()) {
        Some(path) => Ok(PathBuf::from(path)),
        None => Ok(dirs::home_dir()
            .context("Could not locate the local Codex login")?
            .join(".codex")),
    }
}

#[cfg(any(windows, target_os = "macos"))]
fn keyring_login(home: &std::path::Path) -> Result<Option<Vec<u8>>> {
    use sha2::{Digest, Sha256};
    let canonical = home.canonicalize().unwrap_or_else(|_| home.to_path_buf());
    let digest = format!(
        "{:x}",
        Sha256::digest(canonical.to_string_lossy().as_bytes())
    );
    let entry = keyring::Entry::new("Codex Auth", &format!("cli|{}", &digest[..16]))?;
    match entry.get_password() {
        Ok(json) => Ok(Some(json.into_bytes())),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(_) => bail!("Could not read the local Codex login from the credential store. Open Codex or view usage in ChatGPT."),
    }
}

pub(super) fn load(email: Option<&str>, account_id: Option<&str>) -> Result<Credentials> {
    let home = codex_home()?;
    let json = match std::fs::read(home.join("auth.json")) {
        Ok(json) => Some(json),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            #[cfg(any(windows, target_os = "macos"))]
            { keyring_login(&home)? }
            #[cfg(not(any(windows, target_os = "macos")))]
            { None }
        }
        Err(_) => bail!("Could not read the local Codex login. Open Codex or view usage in ChatGPT."),
    }.context("Lathe's ChatGPT connection does not support reading Codex limits. Sign in to Codex with the same ChatGPT account, then refresh, or view usage in ChatGPT.")?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    parse(&json, email, account_id, now)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jwt(value: Value) -> String {
        format!(
            "header.{}.signature",
            URL_SAFE_NO_PAD.encode(value.to_string())
        )
    }

    fn auth(email: &str, expiry: u64) -> Value {
        serde_json::json!({"auth_mode": "chatgpt", "tokens": {
            "id_token": jwt(serde_json::json!({"email": email, "https://api.openai.com/auth": {
                "chatgpt_account_id": "account-123", "chatgpt_account_is_fedramp": true
            }})),
            "access_token": jwt(serde_json::json!({"exp": expiry})),
            "refresh_token": "must-never-be-used"
        }})
    }

    fn parse_value(
        value: Value,
        email: Option<&str>,
        account_id: Option<&str>,
    ) -> Result<Credentials> {
        parse(
            &serde_json::to_vec(&value).unwrap(),
            email,
            account_id,
            1000,
        )
    }

    #[test]
    fn matching_login_recovers_account_metadata() {
        let credentials = parse_value(
            auth("user@example.com", 2000),
            Some("USER@example.com"),
            None,
        )
        .unwrap();
        assert_eq!(credentials.account_id, "account-123");
        assert!(credentials.fedramp_account);
    }

    #[test]
    fn unrelated_or_unidentified_accounts_are_rejected() {
        for (email, account) in [
            (None, None),
            (Some(""), None),
            (Some("other@example.com"), None),
            (Some("user@example.com"), Some("other-workspace")),
        ] {
            assert!(parse_value(auth("user@example.com", 2000), email, account).is_err());
        }
    }

    #[test]
    fn expired_or_invalid_tokens_and_api_key_logins_are_rejected() {
        for expiry in [0, 999, 1060] {
            assert!(parse_value(
                auth("user@example.com", expiry),
                Some("user@example.com"),
                None
            )
            .is_err());
        }
        let mut value = auth("user@example.com", 2000);
        value["auth_mode"] = "apikey".into();
        assert!(parse_value(value, Some("user@example.com"), None).is_err());
        let mut value = auth("user@example.com", 2000);
        value["tokens"]["access_token"] = "secret-opaque-token".into();
        assert!(parse_value(value, Some("user@example.com"), None).is_err());
    }

    #[test]
    fn parse_errors_do_not_expose_credential_values() {
        let error = parse(
            br#"{"tokens":{"access_token":["secret-value"]}}"#,
            None,
            None,
            1000,
        )
        .err()
        .unwrap();
        assert!(!error.to_string().contains("secret-value"));
    }
}
