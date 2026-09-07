use anyhow::{bail, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize};
use std::future::Future;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModelProvider {
    Openai,
    Anthropic,
}

impl ModelProvider {
    pub fn label(self) -> &'static str {
        match self {
            Self::Openai => "ChatGPT",
            Self::Anthropic => "Claude",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "lowercase")]
pub enum ModelSelection {
    Default {
        provider: ModelProvider,
    },
    Fixed {
        provider: ModelProvider,
        model: String,
    },
}

impl Default for ModelSelection {
    fn default() -> Self {
        Self::Default {
            provider: ModelProvider::Openai,
        }
    }
}

impl ModelSelection {
    pub fn provider(&self) -> ModelProvider {
        match self {
            Self::Default { provider } | Self::Fixed { provider, .. } => *provider,
        }
    }

    pub fn validate(&self) -> Result<()> {
        if let Self::Fixed { model, .. } = self {
            if model.trim().is_empty() || model.len() > 200 || model.chars().any(char::is_control) {
                bail!("Select a valid model in Settings");
            }
            let base = model.split('[').next().unwrap_or(model);
            if ["default", "sonnet", "opus", "haiku", "fable", "best"].contains(&base) {
                bail!("A fixed model must use a concrete model ID, not an alias");
            }
        }
        Ok(())
    }
}

// Upgrade the old storage shape without changing a user's explicitly selected model.
pub fn deserialize_selection<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<ModelSelection, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Stored {
        Selection(ModelSelection),
        Legacy(String),
    }
    Ok(match Stored::deserialize(deserializer)? {
        Stored::Selection(selection) => selection,
        Stored::Legacy(model) if model.is_empty() => ModelSelection::default(),
        Stored::Legacy(model) => ModelSelection::Fixed {
            provider: if model.starts_with("claude-") {
                ModelProvider::Anthropic
            } else {
                ModelProvider::Openai
            },
            model,
        },
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderCatalog {
    pub provider: ModelProvider,
    pub models: Vec<ModelInfo>,
    pub default_model: Option<String>,
    pub fetched_at: Option<DateTime<Utc>>,
    pub stale: bool,
    pub error: Option<String>,
}

impl ProviderCatalog {
    pub fn new(
        provider: ModelProvider,
        models: Vec<ModelInfo>,
        default_model: String,
    ) -> Result<Self> {
        if !models.iter().any(|model| model.id == default_model) {
            bail!("{} did not return a usable default model", provider.label());
        }
        Ok(Self {
            provider,
            models,
            default_model: Some(default_model),
            fetched_at: Some(Utc::now()),
            stale: false,
            error: None,
        })
    }

    pub fn resolve_default(&self) -> Result<String> {
        self.default_model.clone().ok_or_else(|| {
            anyhow::anyhow!(
                "{} Default is unavailable. Refresh the model list in Settings.{}",
                self.provider.label(),
                self.error
                    .as_ref()
                    .map(|error| format!(" {error}"))
                    .unwrap_or_default()
            )
        })
    }
}

#[derive(Default)]
pub struct CatalogCache {
    state: Mutex<Option<(Instant, ProviderCatalog)>>,
}

impl CatalogCache {
    pub async fn get<F, Fut>(
        &self,
        provider: ModelProvider,
        force: bool,
        fetch: F,
    ) -> ProviderCatalog
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<ProviderCatalog>>,
    {
        let requested = Instant::now();
        let mut cache = self.state.lock().await;
        if let Some((updated, catalog)) = cache.as_ref() {
            let ttl = if catalog.error.is_some() { 20 } else { 300 };
            if *updated >= requested || (!force && updated.elapsed() < Duration::from_secs(ttl)) {
                return catalog.clone();
            }
        }
        let catalog = match fetch().await {
            Ok(catalog) => catalog,
            Err(error) => {
                let mut catalog =
                    cache
                        .as_ref()
                        .map(|(_, value)| value.clone())
                        .unwrap_or(ProviderCatalog {
                            provider,
                            models: vec![],
                            default_model: None,
                            fetched_at: None,
                            stale: true,
                            error: None,
                        });
                catalog.stale = true;
                catalog.error = Some(error.to_string());
                catalog
            }
        };
        *cache = Some((Instant::now(), catalog.clone()));
        catalog
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Deserialize)]
    struct LegacySettings {
        #[serde(deserialize_with = "deserialize_selection")]
        model: ModelSelection,
    }

    #[test]
    fn legacy_model_remains_fixed_and_provider_is_explicit() {
        let settings: LegacySettings =
            serde_json::from_str(r#"{"model":"claude-opus-4-6"}"#).unwrap();
        assert_eq!(
            settings.model,
            ModelSelection::Fixed {
                provider: ModelProvider::Anthropic,
                model: "claude-opus-4-6".into()
            }
        );
    }

    #[test]
    fn default_never_serializes_a_resolved_model_id() {
        let value = serde_json::to_value(ModelSelection::default()).unwrap();
        assert_eq!(
            value,
            serde_json::json!({"mode":"default","provider":"openai"})
        );
        assert!(ModelSelection::Fixed {
            provider: ModelProvider::Anthropic,
            model: "opus".into()
        }
        .validate()
        .is_err());
    }

    #[tokio::test]
    async fn failed_refresh_retains_the_last_successful_catalog_without_inventing_a_default() {
        let cache = CatalogCache::default();
        let first = cache
            .get(ModelProvider::Openai, false, || async {
                anyhow::bail!("offline")
            })
            .await;
        assert!(first.resolve_default().is_err());
        let good = cache
            .get(ModelProvider::Openai, true, || async {
                ProviderCatalog::new(
                    ModelProvider::Openai,
                    vec![ModelInfo {
                        id: "test-model".into(),
                        name: "Test".into(),
                    }],
                    "test-model".into(),
                )
            })
            .await;
        let stale = cache
            .get(ModelProvider::Openai, true, || async {
                anyhow::bail!("offline")
            })
            .await;
        assert!(stale.stale);
        assert_eq!(good.fetched_at, stale.fetched_at);
        assert_eq!(stale.resolve_default().unwrap(), "test-model");
    }

    #[tokio::test]
    async fn simultaneous_forced_refreshes_share_one_fetch() {
        let cache = CatalogCache::default();
        let calls = std::sync::atomic::AtomicUsize::new(0);
        let fetch = || async {
            calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(20)).await;
            ProviderCatalog::new(
                ModelProvider::Openai,
                vec![ModelInfo {
                    id: "next".into(),
                    name: "Next".into(),
                }],
                "next".into(),
            )
        };
        let (a, b) = tokio::join!(
            cache.get(ModelProvider::Openai, true, fetch),
            cache.get(ModelProvider::Openai, true, fetch)
        );
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(a.default_model, b.default_model);
    }
}
