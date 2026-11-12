//! Models offered per provider (Hydra `electron/agents/ProviderModelCatalog.ts`).
//!
//! * Claude: the built-in aliases (`opus`, `sonnet`, `haiku`).
//! * Codex: asked from `codex app-server --listen stdio://` (JSON-RPC `model/list`).
//! * OpenCode: parsed from `opencode models --verbose`.
//!
//! Discovered lists are cached for 5 minutes; when the CLI is missing, slow or answers nothing
//! useful the built-in list of [`provider_models`] is used (and the failure is remembered for 30 s
//! so a missing CLI is not spawned again on every call).

use std::process::Stdio;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::Mutex;

use super::env::{EnvOverrides, build_agent_env_from_process};
use crate::types::{ProviderId, ProviderModelOption, provider_models};

const MODEL_CACHE_TTL: Duration = Duration::from_secs(5 * 60);
const FAILURE_TTL: Duration = Duration::from_secs(30);
const CODEX_APP_SERVER_TIMEOUT: Duration = Duration::from_secs(8);
const OPENCODE_TIMEOUT: Duration = Duration::from_secs(10);

struct CacheEntry {
    models: Vec<ProviderModelOption>,
    valid_until: Instant,
}

#[derive(Default)]
pub struct ProviderModelCatalog {
    codex: Mutex<Option<CacheEntry>>,
    opencode: Mutex<Option<CacheEntry>>,
    env: parking_lot::Mutex<EnvOverrides>,
}

impl ProviderModelCatalog {
    pub fn new() -> Self {
        Self::default()
    }

    /// Env overrides for the CLIs we spawn (the daemon's fixed PATH).
    pub fn set_env_overrides(&self, overrides: EnvOverrides) {
        *self.env.lock() = overrides;
    }

    pub async fn list(&self, provider: ProviderId) -> Vec<ProviderModelOption> {
        match provider {
            ProviderId::Claude => provider_models(provider),
            ProviderId::Codex => {
                self.cached(&self.codex, provider, |env| {
                    Box::pin(fetch_codex_models(env))
                })
                .await
            }
            ProviderId::Opencode => {
                self.cached(&self.opencode, provider, |env| {
                    Box::pin(fetch_opencode_models(env))
                })
                .await
            }
        }
    }

    async fn cached(
        &self,
        slot: &Mutex<Option<CacheEntry>>,
        provider: ProviderId,
        fetch: impl FnOnce(
            EnvOverrides,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<Vec<ProviderModelOption>>> + Send>,
        >,
    ) -> Vec<ProviderModelOption> {
        // Holding the lock while fetching also dedupes concurrent callers.
        let mut slot = slot.lock().await;
        if let Some(entry) = slot.as_ref()
            && entry.valid_until > Instant::now()
        {
            return entry.models.clone();
        }
        let env = self.env.lock().clone();
        let (models, ttl) = match fetch(env).await {
            Ok(models) => (models, MODEL_CACHE_TTL),
            Err(err) => {
                tracing::debug!(%provider, err = %format!("{err:#}"), "model discovery failed, using the built-in list");
                (provider_models(provider), FAILURE_TTL)
            }
        };
        *slot = Some(CacheEntry {
            models: models.clone(),
            valid_until: Instant::now() + ttl,
        });
        models
    }
}

/// `command args...`, through `cmd.exe /c` on Windows so `.cmd` shims work.
fn cli_command(command: &str, args: &[&str], env: &EnvOverrides) -> Command {
    let mut cmd = if cfg!(windows) {
        let mut c = crate::process::background_tokio_command("cmd.exe");
        c.arg("/c").arg(command).args(args);
        c
    } else {
        let mut c = crate::process::background_tokio_command(command);
        c.args(args);
        c
    };
    cmd.env_clear()
        .envs(build_agent_env_from_process(env))
        .kill_on_drop(true);
    cmd
}

async fn fetch_codex_models(env: EnvOverrides) -> Result<Vec<ProviderModelOption>> {
    let mut cmd = cli_command("codex", &["app-server", "--listen", "stdio://"], &env);
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = cmd.spawn().context("starting `codex app-server`")?;
    let mut stdin = child.stdin.take().context("no stdin")?;
    let stdout = child.stdout.take().context("no stdout")?;
    let mut lines = BufReader::new(stdout).lines();
    let send = |value: Value| format!("{value}\n");

    let run = async {
        stdin
            .write_all(
                send(json!({
                    "jsonrpc": "2.0", "id": "initialize", "method": "initialize",
                    "params": {"clientInfo": {"name": "agent-studio", "version": crate::VERSION}, "capabilities": null}
                }))
                .as_bytes(),
            )
            .await?;
        while let Some(line) = lines.next_line().await? {
            let Ok(message) = serde_json::from_str::<Value>(line.trim()) else {
                continue;
            };
            if let Some(error) = message.get("error") {
                let text = error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("Codex app-server request failed");
                bail!("{text}");
            }
            match message.get("id").and_then(Value::as_str) {
                Some("initialize") => {
                    stdin
                        .write_all(
                            send(json!({
                                "jsonrpc": "2.0", "id": "model-list", "method": "model/list",
                                "params": {"includeHidden": false, "limit": 100}
                            }))
                            .as_bytes(),
                        )
                        .await?;
                }
                Some("model-list") => {
                    let models =
                        parse_codex_model_list(message.get("result").unwrap_or(&Value::Null));
                    if models.is_empty() {
                        bail!("Codex app-server returned an empty model catalog");
                    }
                    return Ok(models);
                }
                _ => {}
            }
        }
        Err(anyhow!("Codex app-server exited before returning models"))
    };
    let result = tokio::time::timeout(CODEX_APP_SERVER_TIMEOUT, run)
        .await
        .map_err(|_| anyhow!("Timed out querying the Codex model catalog"))?;
    let _ = child.start_kill();
    result
}

/// The `result` of the app-server's `model/list` -> visible models.
pub fn parse_codex_model_list(result: &Value) -> Vec<ProviderModelOption> {
    result
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|m| m.get("hidden").and_then(Value::as_bool) != Some(true))
        .filter_map(|m| {
            let id = m.get("model")?.as_str()?.to_string();
            let text = |k: &str| m.get(k).and_then(Value::as_str).map(str::to_string);
            Some(ProviderModelOption {
                label: text("displayName")
                    .filter(|l| !l.is_empty())
                    .unwrap_or_else(|| id.clone()),
                id,
                description: text("description"),
                hidden: false,
                is_default: m.get("isDefault").and_then(Value::as_bool) == Some(true),
                reasoning_efforts: m
                    .get("supportedReasoningEfforts")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|e| e.get("reasoningEffort")?.as_str().map(str::to_string))
                    .collect(),
                default_reasoning_effort: text("defaultReasoningEffort"),
            })
        })
        .collect()
}

async fn fetch_opencode_models(env: EnvOverrides) -> Result<Vec<ProviderModelOption>> {
    let mut cmd = cli_command("opencode", &["models", "--verbose"], &env);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let child = cmd.spawn().context("starting `opencode models`")?;
    let output = tokio::time::timeout(OPENCODE_TIMEOUT, child.wait_with_output())
        .await
        .map_err(|_| anyhow!("Timed out querying the OpenCode model catalog"))??;
    let models = parse_opencode_verbose_output(&String::from_utf8_lossy(&output.stdout));
    if models.is_empty() {
        bail!("OpenCode returned an empty model catalog");
    }
    Ok(models)
}

fn is_model_header(line: &str) -> bool {
    let line = line.trim_end();
    !line.is_empty()
        && !line.starts_with('{')
        && !line.chars().any(char::is_whitespace)
        && line.contains('/')
}

/// Parses `opencode models --verbose`: a `providerID/modelId` header line followed by a JSON block
/// per model. Unparseable blocks are skipped.
pub fn parse_opencode_verbose_output(output: &str) -> Vec<ProviderModelOption> {
    let mut blocks: Vec<(&str, String)> = Vec::new();
    for line in output.lines() {
        if is_model_header(line) {
            blocks.push((line.trim(), String::new()));
        } else if let Some((_, body)) = blocks.last_mut() {
            body.push_str(line);
            body.push('\n');
        }
    }
    let mut models: Vec<ProviderModelOption> = Vec::new();
    for (header, body) in blocks {
        let body = body.trim();
        if !body.starts_with('{') {
            continue;
        }
        let Ok(data) = serde_json::from_str::<Value>(body) else {
            continue;
        };
        let (Some(provider), Some(id)) = (
            data.get("providerID").and_then(Value::as_str),
            data.get("id").and_then(Value::as_str),
        ) else {
            continue;
        };
        let reasoning_efforts: Vec<String> = data
            .get("variants")
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
            .filter(|(_, v)| v.get("reasoningEffort").is_some() || v.get("thinking").is_some())
            .map(|(k, _)| k.clone())
            .collect();
        models.push(ProviderModelOption {
            id: format!("{provider}/{id}"),
            label: data
                .get("name")
                .and_then(Value::as_str)
                .filter(|n| !n.is_empty())
                .unwrap_or(header)
                .to_string(),
            is_default: models.is_empty(),
            reasoning_efforts,
            ..Default::default()
        });
    }
    models
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn parses_codex_model_list_and_hides_hidden_models() {
        let result = json!({"data":[
            {"model":"gpt-5.4","displayName":"GPT-5.4","description":"d","isDefault":true,
             "supportedReasoningEfforts":[{"reasoningEffort":"low"},{"reasoningEffort":"high"}],
             "defaultReasoningEffort":"low"},
            {"model":"secret","displayName":"Secret","hidden":true},
            {"model":"bare"},
            {"nomodel":1}
        ]});
        let models = parse_codex_model_list(&result);
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].id, "gpt-5.4");
        assert_eq!(models[0].label, "GPT-5.4");
        assert!(models[0].is_default);
        assert_eq!(models[0].reasoning_efforts, ["low", "high"]);
        assert_eq!(models[0].default_reasoning_effort.as_deref(), Some("low"));
        assert_eq!(models[1].label, "bare", "label falls back to the id");
        assert!(parse_codex_model_list(&json!({})).is_empty());
    }

    #[test]
    fn parses_opencode_verbose_output() {
        let output = r#"opencode/big-pickle
{
  "id": "big-pickle",
  "providerID": "opencode",
  "name": "Big Pickle",
  "status": "active",
  "variants": {
    "high": { "reasoningEffort": "high" },
    "plain": { "other": 1 }
  }
}
anthropic/claude-sonnet
{
  "id": "claude-sonnet",
  "providerID": "anthropic",
  "name": ""
}
broken/model
{ not json
"#;
        let models = parse_opencode_verbose_output(output);
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].id, "opencode/big-pickle");
        assert_eq!(models[0].label, "Big Pickle");
        assert!(models[0].is_default && !models[1].is_default);
        assert_eq!(models[0].reasoning_efforts, ["high"]);
        assert_eq!(models[1].id, "anthropic/claude-sonnet");
        assert_eq!(
            models[1].label, "anthropic/claude-sonnet",
            "empty name -> header"
        );
        assert!(parse_opencode_verbose_output("garbage\nlines").is_empty());
    }

    #[tokio::test]
    async fn claude_is_static_and_missing_clis_fall_back_to_the_built_in_lists() {
        let catalog = ProviderModelCatalog::new();
        assert_eq!(
            catalog.list(ProviderId::Claude).await,
            provider_models(ProviderId::Claude)
        );
        // No CLIs on an empty PATH: fall back (and remember the failure).
        let mut env = EnvOverrides::new();
        env.insert("PATH".into(), Some("/nonexistent-dir-for-tests".into()));
        catalog.set_env_overrides(env);
        assert_eq!(
            catalog.list(ProviderId::Codex).await,
            provider_models(ProviderId::Codex)
        );
        assert_eq!(
            catalog.list(ProviderId::Opencode).await,
            provider_models(ProviderId::Opencode)
        );
    }
}
