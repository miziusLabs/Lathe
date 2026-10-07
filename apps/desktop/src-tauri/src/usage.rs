//! Account-wide Codex plan limits, using the same usage endpoint as openai/codex.
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use ts_rs::TS;

mod codex;

const USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";
const CODEX_ORIGINATOR: &str = "codex_cli_rs";

#[derive(Debug, Default, Serialize, TS)]
#[ts(export, export_to = "events.ts")]
#[serde(rename_all = "camelCase")]
pub struct PlanUsage {
    pub signed_in: bool,
    pub email: Option<String>,
    pub plan_type: Option<String>,
    // True when limits were read using a matching local Codex login.
    pub from_codex_login: bool,
    pub five_hour: Option<PlanUsageWindow>,
    pub weekly: Option<PlanUsageWindow>,
}

/// Codex rate-limit window, with the reset timestamp in Unix milliseconds.
#[derive(Debug, Serialize, TS)]
#[ts(export, export_to = "events.ts")]
#[serde(rename_all = "camelCase")]
pub struct PlanUsageWindow {
    pub used_percent: f64,
    pub reset_at: i64,
}

#[derive(Deserialize)]
struct UsageResponse {
    plan_type: String,
    rate_limit: Option<Limits>,
}

#[derive(Deserialize)]
struct Limits {
    primary_window: Option<Window>,
    secondary_window: Option<Window>,
}

#[derive(Deserialize)]
struct Window {
    used_percent: f64,
    limit_window_seconds: i64,
    reset_at: i64,
}

fn usage_request(
    client: &reqwest::Client,
    token: &str,
    account_id: Option<&str>,
    fedramp_account: bool,
) -> reqwest::RequestBuilder {
    let request = client
        .get(USAGE_URL)
        .header(reqwest::header::USER_AGENT, "codex-cli")
        .header("originator", CODEX_ORIGINATOR)
        .bearer_auth(token);
    let request = if fedramp_account {
        request.header("X-OpenAI-Fedramp", "true")
    } else {
        request
    };
    match account_id.filter(|id| !id.is_empty()) {
        Some(id) => request.header("ChatGPT-Account-ID", id),
        None => request,
    }
}

fn unavailable_message(status: reqwest::StatusCode) -> String {
    format!(
        "ChatGPT plan usage is unavailable (HTTP {}). Lathe's ChatGPT connection may not support reading Codex limits. Sign in to Codex with the same ChatGPT account, then refresh, or view usage in ChatGPT.",
        status.as_u16()
    )
}

fn apply_limits(usage: &mut PlanUsage, response: UsageResponse) -> Result<()> {
    usage.plan_type = Some(response.plan_type);
    if let Some(limits) = response.rate_limit {
        for window in [limits.primary_window, limits.secondary_window]
            .into_iter()
            .flatten()
        {
            let target = match window.limit_window_seconds {
                18_000 => &mut usage.five_hour,
                604_800 => &mut usage.weekly,
                // Do not mislabel an unfamiliar duration as a five-hour/week limit.
                _ => continue,
            };
            *target = Some(PlanUsageWindow {
                used_percent: window.used_percent.clamp(0.0, 100.0),
                reset_at: window
                    .reset_at
                    .checked_mul(1000)
                    .context("Invalid plan usage reset time")?,
            });
        }
    }
    Ok(())
}

pub async fn fetch() -> Result<PlanUsage> {
    crate::tls::initialize();
    let status = crate::account::status()?;
    let mut usage = PlanUsage {
        signed_in: status.signed_in,
        email: status.email,
        ..PlanUsage::default()
    };
    if !usage.signed_in {
        return Ok(usage);
    }
    let token = crate::account::access_token().await?;
    let account_id = crate::account::account_id()?;
    let fedramp_account = crate::account::is_fedramp_account()?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()?;
    let mut response = usage_request(&client, &token, account_id.as_deref(), fedramp_account)
        .send()
        .await
        .context("Could not fetch ChatGPT plan usage")?;
    // SIWC tokens authorize api.openai.com, but are not necessarily admitted to
    // the Codex usage endpoint. Codex owns its credentials and refresh lifecycle.
    if matches!(response.status().as_u16(), 401 | 403) {
        let credentials = codex::load(usage.email.as_deref(), account_id.as_deref())?;
        response = usage_request(
            &client,
            &credentials.access_token,
            Some(&credentials.account_id),
            credentials.fedramp_account,
        )
        .send()
        .await
        .context("Could not fetch usage with the local Codex login")?;
        usage.from_codex_login = true;
        if matches!(response.status().as_u16(), 401 | 403) {
            anyhow::bail!("The local Codex login cannot read usage (HTTP {}). Open Codex to refresh its login, then refresh here, or view usage in ChatGPT.", response.status().as_u16());
        }
    }
    if !response.status().is_success() {
        anyhow::bail!("{}", unavailable_message(response.status()));
    }
    let response: UsageResponse = response
        .json()
        .await
        .context("ChatGPT returned an unfamiliar plan usage response")?;
    apply_limits(&mut usage, response)?;
    Ok(usage)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_windows_by_duration_and_normalizes_percent_and_reset() {
        let response = serde_json::from_value(serde_json::json!({
            "plan_type": "plus",
            "rate_limit": {
                "primary_window": {"used_percent": 120, "limit_window_seconds": 604800, "reset_at": 1700000000},
                "secondary_window": {"used_percent": -5, "limit_window_seconds": 18000, "reset_at": 1700000100}
            }
        })).unwrap();
        let mut usage = PlanUsage::default();
        apply_limits(&mut usage, response).unwrap();
        assert_eq!(usage.plan_type.as_deref(), Some("plus"));
        assert_eq!(usage.weekly.unwrap().used_percent, 100.0);
        let short = usage.five_hour.unwrap();
        assert_eq!(short.used_percent, 0.0);
        assert_eq!(short.reset_at, 1700000100000);
    }

    #[test]
    fn missing_or_unknown_limits_are_not_reported_as_full_capacity() {
        for limits in [
            serde_json::Value::Null,
            serde_json::json!({
                "primary_window": {"used_percent": 12, "limit_window_seconds": 3600, "reset_at": 1700000000}
            }),
        ] {
            let response = serde_json::from_value(
                serde_json::json!({"plan_type": "free", "rate_limit": limits}),
            )
            .unwrap();
            let mut usage = PlanUsage::default();
            apply_limits(&mut usage, response).unwrap();
            assert!(usage.five_hour.is_none());
            assert!(usage.weekly.is_none());
        }
    }

    #[test]
    fn request_uses_bearer_and_account_routing_without_reserve() {
        crate::tls::initialize();
        let token = "test-access-token";
        let request = usage_request(&reqwest::Client::new(), token, Some("account-123"), true)
            .build()
            .unwrap();
        assert_eq!(request.method(), reqwest::Method::GET);
        assert_eq!(request.url().as_str(), USAGE_URL);
        assert_eq!(
            request.headers()["authorization"],
            "Bearer test-access-token"
        );
        assert_eq!(request.headers()["user-agent"], "codex-cli");
        assert_eq!(request.headers()["originator"], CODEX_ORIGINATOR);
        assert_eq!(request.headers()["x-openai-fedramp"], "true");
        assert_eq!(request.headers()["chatgpt-account-id"], "account-123");
        assert!(!request
            .headers()
            .contains_key("x-openai-codex-luna-reserve"));
        let request_without_routing = usage_request(&reqwest::Client::new(), "token", None, false)
            .build()
            .unwrap();
        assert!(!request_without_routing
            .headers()
            .contains_key("chatgpt-account-id"));
        assert!(!request_without_routing
            .headers()
            .contains_key("x-openai-fedramp"));
    }

    #[test]
    fn unavailable_guidance_does_not_blame_missing_routing_metadata() {
        let error = unavailable_message(reqwest::StatusCode::UNAUTHORIZED);
        assert!(error.contains("same ChatGPT account"));
        assert!(!error.contains("Reconnect in Settings"));
    }

    /// Explicitly opt in: reads local credentials and makes read-only usage GETs.
    #[tokio::test]
    #[ignore = "requires a connected ChatGPT account and matching local Codex login"]
    async fn live_plan_usage() {
        let usage = fetch().await.unwrap();
        assert!(usage.signed_in);
        assert!(usage.plan_type.is_some());
        assert!(usage.five_hour.is_some() || usage.weekly.is_some());
    }
}
