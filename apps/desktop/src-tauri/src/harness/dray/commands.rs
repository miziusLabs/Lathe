//! Account-specific OpenAI model catalog and native skill discovery.
use crate::models::{AgentModel, Effort, Model, ModelId};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    future::Future,
    sync::{
        atomic::{AtomicU64, Ordering},
        OnceLock,
    },
    time::{Duration, Instant},
};
use tokio::sync::Mutex;
use ts_rs::TS;

const CATALOG_TTL: Duration = Duration::from_secs(300);
static CATALOG: OnceLock<Mutex<CatalogCache>> = OnceLock::new();
static ACCOUNT_GENERATION: AtomicU64 = AtomicU64::new(0);

#[derive(Default)]
struct CatalogCache {
    entry: Option<(u64, Instant, Vec<Model>)>,
}

impl CatalogCache {
    async fn get_or_fetch<F, Fut>(&mut self, generation: u64, fetch: F) -> Result<Vec<Model>>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<Vec<Model>>>,
    {
        if let Some((cached_generation, fetched_at, models)) = &self.entry {
            if *cached_generation == generation && fetched_at.elapsed() < CATALOG_TTL {
                return Ok(models.clone());
            }
        }
        let models = fetch().await?;
        self.entry = Some((generation, Instant::now(), models.clone()));
        Ok(models)
    }
}

// Generation-based invalidation avoids taking the catalog lock while OAuth
// holds its own lock; a catalog fetch may itself need to refresh OAuth.
pub fn invalidate_model_catalog() {
    ACCOUNT_GENERATION.fetch_add(1, Ordering::SeqCst);
}

// The catalog is gated by client compatibility, independently of Lathe's app
// version. Omitting this returns a legacy catalog that excludes newer models.
const CATALOG_CLIENT_VERSION: &str = "0.159.0";

fn catalog_request(client: &reqwest::Client, token: &str) -> reqwest::RequestBuilder {
    client
        .get(format!(
            "{}/models?client_version={CATALOG_CLIENT_VERSION}",
            lathe_agent::API
        ))
        .bearer_auth(token)
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "events.ts")]
#[serde(rename_all = "camelCase")]
pub struct SlashCommand {
    pub name: String,
    pub description: String,
    pub argument_hint: String,
    pub aliases: Vec<String>,
    pub is_skill: bool,
}

pub async fn list_commands(cwd: &str) -> Result<Vec<SlashCommand>> {
    Ok(lathe_agent::skills::discover(std::path::Path::new(cwd))
        .into_iter()
        .map(|skill| SlashCommand {
            name: skill.name,
            description: skill.description,
            argument_hint: String::new(),
            aliases: Vec::new(),
            is_skill: true,
        })
        .collect())
}

pub async fn list_models(_cwd: Option<&str>) -> Result<Vec<Model>> {
    if !crate::account::status()?.signed_in {
        return Ok(Vec::new());
    }
    // Share the picker/title/send catalog and coalesce concurrent cold loads.
    // Keep account checks outside the cache so signing out never permits sends.
    let mut cache = CATALOG
        .get_or_init(|| Mutex::new(CatalogCache::default()))
        .lock()
        .await;
    let generation = ACCOUNT_GENERATION.load(Ordering::SeqCst);
    cache
        .get_or_fetch(generation, || async {
            let token = crate::account::access_token().await?;
            crate::tls::initialize();
            let client = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(20))
                .build()?;
            let response = catalog_request(&client, &token)
                .send()
                .await?
                .error_for_status()?;
            let body: Value = response.json().await?;
            models_from_response(&body)
        })
        .await
}

pub fn models_from_response(body: &Value) -> Result<Vec<Model>> {
    let models = body["models"]
        .as_array()
        .context("OpenAI returned an unfamiliar model catalog")?;
    Ok(models
        .iter()
        .filter(|model| model["visibility"] == "list")
        .filter_map(|model| {
            let id = model["slug"].as_str()?;
            let efforts = model["supported_reasoning_levels"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|level| {
                    let value = level.as_str().or_else(|| level["effort"].as_str())?;
                    // `ultra` is Codex's automatic delegation mode, not a
                    // Responses reasoning effort supported by our runtime.
                    Effort::from_arg(value)
                })
                .collect::<Vec<_>>();
            let default_effort = model["default_reasoning_level"]
                .as_str()
                .and_then(Effort::from_arg)
                .filter(|e| efforts.contains(e))
                .or_else(|| efforts.first().copied());
            Some(Model {
                id: ModelId::Dray,
                agent_model: Some(AgentModel {
                    provider: "openai".into(),
                    id: id.into(),
                }),
                label: model["display_name"].as_str().unwrap_or(id).into(),
                efforts,
                default_effort,
                context_window: model["context_window"].as_u64(),
            })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn catalog_reuses_models_until_expiry_or_account_change() {
        let mut cache = CatalogCache::default();
        let models = models_from_response(&serde_json::json!({"models": [
            {"slug": "account-model", "visibility": "list"}
        ]}))
        .unwrap();
        cache
            .get_or_fetch(0, || async { Ok(models) })
            .await
            .unwrap();
        let cached = cache
            .get_or_fetch(0, || async {
                panic!("a warm catalog must not make a network request")
            })
            .await
            .unwrap();
        assert_eq!(cached[0].agent_model.as_ref().unwrap().id, "account-model");

        // A new account must not inherit the previous account's catalog.
        assert!(cache
            .get_or_fetch(1, || async { Ok(Vec::new()) })
            .await
            .unwrap()
            .is_empty());
        cache.entry.as_mut().unwrap().1 = Instant::now() - CATALOG_TTL;
        let refreshed = cache
            .get_or_fetch(1, || async { Ok(cached) })
            .await
            .unwrap();
        assert_eq!(refreshed.len(), 1);
    }

    #[tokio::test]
    async fn catalog_failures_are_not_cached_or_served_as_stale_success() {
        let mut cache = CatalogCache::default();
        cache
            .get_or_fetch(0, || async { Ok(Vec::new()) })
            .await
            .unwrap();
        cache.entry.as_mut().unwrap().1 = Instant::now() - CATALOG_TTL;
        assert!(cache
            .get_or_fetch(0, || async { anyhow::bail!("catalog unavailable") })
            .await
            .is_err());
        let mut fetched = false;
        cache
            .get_or_fetch(0, || async {
                fetched = true;
                Ok(Vec::new())
            })
            .await
            .unwrap();
        assert!(fetched);
    }

    #[tokio::test]
    async fn concurrent_catalog_loads_fetch_only_once() {
        let cache = std::sync::Arc::new(Mutex::new(CatalogCache::default()));
        let fetches = std::sync::Arc::new(AtomicU64::new(0));
        let mut tasks = tokio::task::JoinSet::new();
        for _ in 0..8 {
            let cache = cache.clone();
            let fetches = fetches.clone();
            tasks.spawn(async move {
                cache
                    .lock()
                    .await
                    .get_or_fetch(0, || async {
                        fetches.fetch_add(1, Ordering::SeqCst);
                        tokio::task::yield_now().await;
                        Ok(Vec::new())
                    })
                    .await
                    .unwrap();
            });
        }
        while let Some(result) = tasks.join_next().await {
            result.unwrap();
        }
        assert_eq!(fetches.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn catalog_request_includes_model_catalog_compatibility_version() {
        crate::tls::initialize();
        let request = catalog_request(&reqwest::Client::new(), "test-token")
            .build()
            .unwrap();
        assert_eq!(
            request.url().as_str(),
            "https://api.openai.com/v1/models?client_version=0.159.0"
        );
    }

    #[test]
    fn new_models_keep_their_own_reasoning_options_and_defaults() {
        let models = models_from_response(&serde_json::json!({"models": [
            {"slug":"gpt-6.1-sol","visibility":"list","supported_reasoning_levels":["low","medium","high","xhigh","max","ultra"],"default_reasoning_level":"low"},
            {"slug":"gpt-6-astra","visibility":"list","supported_reasoning_levels":[{"effort":"low"},{"effort":"medium"},{"effort":"high"},{"effort":"xhigh"},{"effort":"max"},{"effort":"ultra"}],"default_reasoning_level":"low"},
            {"slug":"gpt-6-luna","visibility":"list","supported_reasoning_levels":["low","medium","high","xhigh","max"],"default_reasoning_level":"medium"},
            {"slug":"hidden-model","visibility":"hide"}
        ]})).unwrap();
        assert_eq!(
            models
                .iter()
                .map(|m| m.agent_model.as_ref().unwrap().id.as_str())
                .collect::<Vec<_>>(),
            vec!["gpt-6.1-sol", "gpt-6-astra", "gpt-6-luna"]
        );
        assert_eq!(
            models[1].efforts,
            vec![
                Effort::Low,
                Effort::Medium,
                Effort::High,
                Effort::Xhigh,
                Effort::Max
            ]
        );
        assert_eq!(models[0].default_effort, Some(Effort::Low));
        assert_eq!(models[2].default_effort, Some(Effort::Medium));
        assert_eq!(
            crate::models::resolve_effort(&models[1], Some(Effort::None)),
            Some(Effort::Low)
        );
        assert_eq!(
            crate::models::resolve_effort(&models[1], Some(Effort::Minimal)),
            Some(Effort::Low)
        );
    }

    #[test]
    fn missing_or_unsupported_defaults_use_the_first_supported_effort() {
        let models = models_from_response(&serde_json::json!({"models": [
            {"slug":"future-model","visibility":"list","supported_reasoning_levels":["high","unrecognized"],"default_reasoning_level":"unrecognized"},
            {"slug":"no-default","visibility":"list","supported_reasoning_levels":["medium"]}
        ]})).unwrap();
        assert_eq!(models[0].default_effort, Some(Effort::High));
        assert_eq!(models[1].default_effort, Some(Effort::Medium));
    }
    #[test]
    fn catalog_uses_visible_account_models_and_server_reasoning_levels() {
        let models=models_from_response(&serde_json::json!({"models":[
            {"slug":"model-b","display_name":"Model B","visibility":"list","supported_reasoning_levels":[{"effort":"low"},{"effort":"high"}],"default_reasoning_level":"high"},
            {"slug":"hidden","visibility":"hidden"},
            {"slug":"model-a","visibility":"list"}
        ]})).unwrap();
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].label, "Model B");
        assert_eq!(models[0].efforts, vec![Effort::Low, Effort::High]);
        assert!(models[1].efforts.is_empty());
    }
}
