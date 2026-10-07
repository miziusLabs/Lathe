//! ChatGPT OAuth for Lathe's own registered open-source client.
//! Credentials stay in the native backend and OS credential store.
use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use jsonwebtoken::{decode, decode_header, jwk::JwkSet, DecodingKey, Validation};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    sync::OnceLock,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Emitter};
use tauri_plugin_opener::OpenerExt;
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::TcpListener,
    sync::Mutex,
};

const ISSUER: &str = "https://auth.openai.com";
const TOKEN: &str = "https://auth.openai.com/api/accounts/oauth/token";
const RESOURCE: &str = "https://api.openai.com/v1";
static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
static PENDING: OnceLock<Mutex<Option<tokio::task::JoinHandle<()>>>> = OnceLock::new();

#[cfg(any(windows, test))]
mod credential_store;

#[derive(Clone, Serialize, Deserialize)]
struct Credentials {
    client_id: String,
    subject: String,
    #[serde(default)]
    account_id: Option<String>,
    email: Option<String>,
    id_token: String,
    access_token: String,
    refresh_token: String,
    scope: String,
    expires_at: u64,
}
#[derive(Deserialize)]
struct Tokens {
    access_token: String,
    refresh_token: Option<String>,
    id_token: Option<String>,
    account_id: Option<String>,
    scope: Option<String>,
    expires_in: u64,
}
#[derive(Serialize, Clone, Default, ts_rs::TS)]
#[ts(export, export_to = "events.ts")]
#[serde(rename_all = "camelCase")]
pub struct AccountStatus {
    pub signed_in: bool,
    pub email: Option<String>,
    pub error: Option<String>,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn random() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}
#[cfg(any(windows, target_os = "macos"))]
fn credential_service() -> &'static str {
    if std::env::var("DRAY_PROFILE").is_ok_and(|profile| profile == "development") {
        "com.mizius.lathe.dev.chatgpt"
    } else {
        "com.mizius.lathe.chatgpt"
    }
}

fn http() -> Result<reqwest::Client> {
    crate::tls::initialize();
    Ok(reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()?)
}

#[cfg(windows)]
fn load() -> Result<Option<Credentials>> {
    credential_store::load(&credential_store::NativeStore)?
        .map(|json| serde_json::from_str(&json).map_err(Into::into))
        .transpose()
}
#[cfg(windows)]
fn store(value: Option<&Credentials>) -> Result<()> {
    credential_store::store(
        &credential_store::NativeStore,
        value.map(serde_json::to_string).transpose()?.as_deref(),
    )
}
#[cfg(target_os = "macos")]
fn load() -> Result<Option<Credentials>> {
    match keyring::Entry::new(credential_service(), "account")?.get_password() {
        Ok(json) => Ok(Some(serde_json::from_str(&json)?)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(e.into()),
    }
}
#[cfg(target_os = "macos")]
fn store(value: Option<&Credentials>) -> Result<()> {
    let entry = keyring::Entry::new(credential_service(), "account")?;
    match value {
        Some(value) => entry.set_password(&serde_json::to_string(value)?)?,
        None => match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => {}
            Err(e) => return Err(e.into()),
        },
    }
    Ok(())
}
#[cfg(not(any(windows, target_os = "macos")))]
fn load() -> Result<Option<Credentials>> {
    let path = crate::store::app_home_dir()?.join("chatgpt.json");
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}
#[cfg(not(any(windows, target_os = "macos")))]
fn store(value: Option<&Credentials>) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let dir = crate::store::app_home_dir()?;
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("chatgpt.json");
    if let Some(value) = value {
        let temp = dir.join(format!("chatgpt-{}.tmp", uuid::Uuid::new_v4()));
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&temp)?;
        file.write_all(&serde_json::to_vec(value)?)?;
        file.sync_all()?;
        std::fs::rename(temp, path)?;
    } else if path.exists() {
        std::fs::remove_file(path)?;
    }
    Ok(())
}

pub fn status() -> Result<AccountStatus> {
    Ok(match load()? {
        Some(c) if !c.access_token.is_empty() => AccountStatus {
            signed_in: true,
            email: c.email,
            error: None,
        },
        _ => AccountStatus::default(),
    })
}

fn account_id_from_claims(claims: &Value) -> Option<String> {
    claims["chatgpt_account_id"]
        .as_str()
        .or_else(|| claims["https://api.openai.com/auth"]["chatgpt_account_id"].as_str())
        .filter(|id| !id.trim().is_empty())
        .map(str::to_owned)
        .or_else(|| {
            claims["organizations"]
                .as_array()?
                .iter()
                .filter_map(|organization| organization["id"].as_str())
                .find(|id| !id.trim().is_empty())
                .map(str::to_owned)
        })
}

fn jwt_claims(token: &str) -> Option<Value> {
    let payload = URL_SAFE_NO_PAD.decode(token.split('.').nth(1)?).ok()?;
    serde_json::from_slice(&payload).ok()
}

fn verified_claims(id_token: &str) -> Option<Value> {
    jwt_claims(id_token)
}

// The stored ID token was verified during sign-in. These claims are used only
// for request routing, never as authorization.
fn verified_account_id(id_token: &str) -> Option<String> {
    account_id_from_claims(&verified_claims(id_token)?)
}

fn account_id_from_access_token(access_token: &str) -> Option<String> {
    account_id_from_claims(&jwt_claims(access_token)?)
}

fn verified_is_fedramp_account(id_token: &str) -> bool {
    verified_claims(id_token)
        .and_then(|claims| {
            claims["https://api.openai.com/auth"]["chatgpt_account_is_fedramp"].as_bool()
        })
        .unwrap_or(false)
}

fn account_id_from_credentials(credentials: &Credentials) -> Option<String> {
    if credentials.access_token.is_empty() {
        return None;
    }
    credentials
        .account_id
        .as_deref()
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
        .or_else(|| verified_account_id(&credentials.id_token))
        // Older OAuth responses may omit `account_id` and the ID token may not contain
        // the account claim. Codex access tokens also carry it in their JWT claims.
        // This is used only to select the account for routing; the bearer token remains
        // the authorization credential and is still validated by the usage endpoint.
        .or_else(|| account_id_from_access_token(&credentials.access_token))
}

pub fn account_id() -> Result<Option<String>> {
    Ok(load()?.as_ref().and_then(account_id_from_credentials))
}

pub fn is_fedramp_account() -> Result<bool> {
    Ok(load()?
        .filter(|credentials| !credentials.access_token.is_empty())
        .is_some_and(|credentials| verified_is_fedramp_account(&credentials.id_token)))
}

pub async fn access_token() -> Result<String> {
    let _guard = LOCK.get_or_init(|| Mutex::new(())).lock().await;
    let mut c = load()?.context("Continue with ChatGPT in Settings to start coding.")?;
    if c.access_token.is_empty() {
        bail!("Continue with ChatGPT in Settings to start coding.");
    }
    if c.expires_at <= now() + 60 {
        let response = http()?
            .post(TOKEN)
            .form(&[
                ("grant_type", "refresh_token"),
                ("client_id", c.client_id.as_str()),
                ("refresh_token", c.refresh_token.as_str()),
                ("resource", RESOURCE),
            ])
            .send()
            .await?;
        if !response.status().is_success() {
            bail!("ChatGPT connection expired. Continue with ChatGPT in Settings to reconnect.");
        }
        let tokens: Tokens = response.json().await?;
        c.access_token = tokens.access_token;
        c.refresh_token = tokens
            .refresh_token
            .context("OpenAI did not return a replacement refresh token")?;
        c.scope = tokens.scope.unwrap_or(c.scope);
        if let Some(account_id) = tokens.account_id.filter(|id| !id.is_empty()) {
            c.account_id = Some(account_id);
        }
        c.expires_at = now() + tokens.expires_in;
        store(Some(&c))?;
    }
    if !c
        .scope
        .split_whitespace()
        .any(|s| s == "chatgpt.tokens.use.direct")
    {
        bail!("Authorize ChatGPT plan usage when connecting Lathe.");
    }
    Ok(c.access_token)
}

pub async fn cancel() {
    if let Some(task) = PENDING.get_or_init(|| Mutex::new(None)).lock().await.take() {
        task.abort();
        let _ = task.await;
    }
}
pub async fn sign_out(app: &AppHandle) -> Result<()> {
    cancel().await;
    let _guard = LOCK.get_or_init(|| Mutex::new(())).lock().await;
    let mut next = AccountStatus::default();
    if let Some(mut credentials) = load()? {
        let refresh_token = std::mem::take(&mut credentials.refresh_token);
        credentials.access_token.clear();
        credentials.id_token.clear();
        credentials.account_id = None;
        store(Some(&credentials))?;
        crate::harness::dray::commands::invalidate_model_catalog();
        let revoked = async {
            let config: Value = http()?
                .get(format!("{ISSUER}/.well-known/openid-configuration"))
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?;
            let endpoint = config["revocation_endpoint"]
                .as_str()
                .context("OpenAI revocation endpoint unavailable")?;
            let endpoint = reqwest::Url::parse(endpoint)?;
            if endpoint.scheme() != "https" || endpoint.host_str() != Some("auth.openai.com") {
                bail!("Unexpected revocation endpoint");
            }
            http()?
                .post(endpoint)
                .form(&[
                    ("token", refresh_token.as_str()),
                    ("token_type_hint", "refresh_token"),
                    ("client_id", credentials.client_id.as_str()),
                ])
                .send()
                .await?
                .error_for_status()?;
            Ok::<_, anyhow::Error>(())
        }
        .await;
        if revoked.is_err() {
            next.error=Some("Signed out locally. Remote disconnection could not be confirmed; disconnect Lathe in ChatGPT Settings if needed.".into());
        }
    }
    app.emit("account_changed", next)?;
    Ok(())
}

fn callback(
    url: &reqwest::Url,
    state: &str,
    returning: Option<&Credentials>,
) -> Result<(String, String)> {
    if url.path() != "/auth/callback" {
        bail!("Invalid callback path");
    }
    let pairs: HashMap<_, _> = url.query_pairs().into_owned().collect();
    if pairs.get("state").map(String::as_str) != Some(state) {
        bail!("Sign-in state could not be verified");
    }
    if pairs.contains_key("error") {
        bail!("ChatGPT sign-in was declined. Try again and allow plan usage.");
    }
    let client = match returning {
        Some(saved) => {
            if pairs
                .get("client_id")
                .is_some_and(|id| id != &saved.client_id)
            {
                bail!("ChatGPT registration changed unexpectedly");
            }
            saved.client_id.clone()
        }
        None => pairs
            .get("client_id")
            .filter(|id| id.starts_with("oaiapp_"))
            .context("ChatGPT registration returned no issued client ID")?
            .clone(),
    };
    Ok((
        pairs
            .get("code")
            .context("Missing authorization code")?
            .clone(),
        client,
    ))
}

pub async fn begin(app: AppHandle) -> Result<()> {
    cancel().await;
    let returning = load()?;
    let dir = crate::store::get_home_app_dir().await?;
    let host_file = dir.join("openai-host-id");
    let host = match tokio::fs::read_to_string(&host_file).await {
        Ok(value) => value,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let value = format!("urn:uuid:{}", uuid::Uuid::new_v4());
            tokio::fs::write(host_file, &value).await?;
            value
        }
        Err(e) => return Err(e.into()),
    };
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let redirect = format!(
        "http://127.0.0.1:{}/auth/callback",
        listener.local_addr()?.port()
    );
    let state = random();
    let nonce = random();
    let verifier = random();
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let mut url = reqwest::Url::parse("https://auth.openai.com/api/accounts/authorize")?;
    {
        let mut query = url.query_pairs_mut();
        query.extend_pairs([
            (
                "client_id",
                returning
                    .as_ref()
                    .map(|c| c.client_id.as_str())
                    .unwrap_or("dynamic_agent_client"),
            ),
            ("ext_agent_host_id", &host),
            ("response_type", "code"),
            ("redirect_uri", &redirect),
            (
                "scope",
                "openid profile email offline_access resource.invoke chatgpt.tokens.use.direct",
            ),
            ("resource", RESOURCE),
            ("state", &state),
            ("nonce", &nonce),
            ("code_challenge_method", "S256"),
            ("code_challenge", &challenge),
        ]);
        if returning.is_none() {
            query.append_pair("agent_name_hint", "Lathe");
        } else if let Some(c) = &returning {
            if !c.id_token.is_empty() {
                query.append_pair("id_token_hint", &c.id_token);
            }
        }
    }
    let event_app = app.clone();
    let task = tokio::spawn(async move {
        let result=tokio::time::timeout(Duration::from_secs(300),async {
            loop {
                let (socket,_)=listener.accept().await?;
                let mut reader=BufReader::new(socket);
                let mut line=String::new();
                let mut bounded = (&mut reader).take(8193);
                tokio::time::timeout(Duration::from_secs(5),bounded.read_line(&mut line)).await??;
                drop(bounded);
                if line.len() > 8192 { bail!("Sign-in callback too large"); }
                let request=line.split_whitespace().nth(1).context("Invalid sign-in callback")?;
                if request.len()>8192 { bail!("Sign-in callback too large"); }
                let callback_url=reqwest::Url::parse(&format!("http://127.0.0.1{request}"))?;
                if callback_url.path()!="/auth/callback" { reader.get_mut().write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await?; continue; }
                let result=async {
                    let (code,client_id)=callback(&callback_url,&state,returning.as_ref())?;
                    let response=http()?.post(TOKEN).form(&[("grant_type","authorization_code"),("client_id",&client_id),("code",&code),("code_verifier",&verifier),("redirect_uri",&redirect),("resource",RESOURCE)]).send().await?;
                    if !response.status().is_success() { bail!("ChatGPT sign-in could not be completed. Start a new sign-in attempt."); }
                    let tokens: Tokens=response.json().await?;
                    let id_token=tokens.id_token.context("Missing ChatGPT identity token")?;
                    let header=decode_header(&id_token)?;
                    if header.alg!=jsonwebtoken::Algorithm::RS256 { bail!("Unexpected ChatGPT identity signature algorithm"); }
                    let keys: JwkSet=http()?.get(format!("{ISSUER}/.well-known/jwks.json")).send().await?.error_for_status()?.json().await?;
                    let key=keys.find(header.kid.as_deref().context("Missing signing key ID")?).context("Unknown ChatGPT signing key")?;
                    let mut validation=Validation::new(jsonwebtoken::Algorithm::RS256);
                    validation.set_issuer(&[ISSUER]); validation.set_audience(&[&client_id]);
                    validation.set_required_spec_claims(&["exp","iss","aud","sub"]);
                    let claims=decode::<Value>(&id_token,&DecodingKey::from_jwk(key)?,&validation)?.claims;
                    if claims["nonce"].as_str()!=Some(nonce.as_str()) { bail!("ChatGPT sign-in nonce could not be verified"); }
                    let subject=claims["sub"].as_str().context("Missing ChatGPT account identity")?.to_string();
                    if returning.as_ref().is_some_and(|c|c.subject!=subject) { bail!("ChatGPT account changed. Sign out before connecting another account."); }
                    let scope=tokens.scope.context("Missing ChatGPT permissions")?;
                    if !scope.split_whitespace().any(|s|s=="chatgpt.tokens.use.direct") { bail!("Allow ChatGPT plan usage to use Lathe's agent."); }
                    let account_id=tokens.account_id.or_else(||account_id_from_claims(&claims));
                    let credentials=Credentials { client_id,subject,account_id,email:claims["email"].as_str().map(str::to_string),id_token,access_token:tokens.access_token,refresh_token:tokens.refresh_token.context("Missing ChatGPT refresh token")?,scope,expires_at:now()+tokens.expires_in };
                    let _guard=LOCK.get_or_init(||Mutex::new(())).lock().await;
                    store(Some(&credentials))?;
                    crate::harness::dray::commands::invalidate_model_catalog();
                    status()
                }.await;
                let body=if result.is_ok() { "Connected to ChatGPT. You can close this window and return to Lathe." } else { "Sign-in failed. Return to Lathe for details and try again." };
                let _=reader.get_mut().write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await;
                return result;
            }
        }).await;
        let next = match result {
            Ok(Ok(status)) => status,
            other => {
                let mut s = status().unwrap_or_default();
                s.error = Some(match other {
                    Ok(Err(e)) => e.to_string(),
                    Err(_) => "ChatGPT sign-in timed out. Try again.".into(),
                    _ => unreachable!(),
                });
                s
            }
        };
        let _ = event_app.emit("account_changed", next);
    });
    *PENDING.get_or_init(|| Mutex::new(None)).lock().await = Some(task);
    if let Err(error) = app.opener().open_url(url.as_str(), None::<&str>) {
        cancel().await;
        return Err(error.into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn account_id_comes_from_oauth_response_or_verified_identity_claim() {
        let response: Tokens = serde_json::from_value(serde_json::json!({
            "access_token": "access", "expires_in": 3600, "account_id": "response-account"
        }))
        .unwrap();
        assert_eq!(response.account_id.as_deref(), Some("response-account"));

        let id_token = format!(
            "header.{}.signature",
            URL_SAFE_NO_PAD.encode(
                serde_json::json!({
                    "https://api.openai.com/auth": {
                        "chatgpt_account_id": "claim-account",
                        "chatgpt_account_is_fedramp": true
                    }
                })
                .to_string()
            )
        );
        assert_eq!(
            verified_account_id(&id_token).as_deref(),
            Some("claim-account")
        );
        assert!(verified_is_fedramp_account(&id_token));
        assert_eq!(verified_account_id("invalid.token"), None);
        assert!(!verified_is_fedramp_account("invalid.token"));
        assert_eq!(account_id_from_claims(&serde_json::json!({})), None);
    }

    #[test]
    fn existing_credentials_ignore_retired_oauth_picture_metadata() {
        let credentials: Credentials = serde_json::from_value(serde_json::json!({
            "client_id": "oaiapp_dray", "subject": "account-1", "email": null,
            "picture": "https://example.com/avatar.png",
            "id_token": "identity", "access_token": "access", "refresh_token": "refresh",
            "scope": "openid profile", "expires_at": 0
        }))
        .unwrap();
        assert!(credentials.account_id.is_none());
    }

    #[test]
    fn account_id_requires_nonempty_explicit_metadata_or_verified_claim() {
        let mut credentials: Credentials = serde_json::from_value(serde_json::json!({
            "client_id": "oaiapp_dray",
            "subject": "account-1",
            "account_id": "",
            "email": null,
            "id_token": "invalid.token",
            "access_token": "access",
            "refresh_token": "refresh",
            "scope": "openid profile",
            "expires_at": 0
        }))
        .unwrap();
        assert_eq!(account_id_from_credentials(&credentials), None);

        credentials.account_id = Some("explicit-account-id".into());
        assert_eq!(
            account_id_from_credentials(&credentials).as_deref(),
            Some("explicit-account-id")
        );
        credentials.access_token.clear();
        assert_eq!(account_id_from_credentials(&credentials), None);
    }

    #[test]
    fn callback_requires_state_code_and_issued_registration() {
        let url = reqwest::Url::parse(
            "http://127.0.0.1:1234/auth/callback?state=good&code=code&client_id=oaiapp_dray",
        )
        .unwrap();
        assert_eq!(
            callback(&url, "good", None).unwrap(),
            ("code".into(), "oaiapp_dray".into())
        );
        assert!(callback(&url, "wrong", None).is_err());
        assert!(callback(&reqwest::Url::parse("http://127.0.0.1/auth/callback?state=good&code=code&client_id=dynamic_agent_client").unwrap(),"good",None).is_err());
        assert!(callback(
            &reqwest::Url::parse("http://127.0.0.1/auth/callback?state=good&error=access_denied")
                .unwrap(),
            "good",
            None
        )
        .is_err());
    }
}
