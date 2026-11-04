//! Thin wrapper turning [`Effect`]s into daemon requests and their results into [`Msg`]s.
//!
//! Keystrokes, pasted text and resizes go through one worker so they reach the daemon in the
//! order they happened (requests of a connection are answered in order, but spawning one task
//! per request would not preserve it). Everything else runs in its own task.

use std::sync::Arc;

use as_proto::DaemonClient;
use tokio::sync::mpsc;

use crate::app::{Effect, Msg};

/// Sessions asked of the daemon for the picker (the filters work on this list).
const SESSION_LIST_LIMIT: usize = 3000;

enum Ordered {
    Raw(String, Vec<u8>),
    Input(String, String),
    Resize(String, u16, u16),
    Focus(bool),
}

pub struct Io {
    client: Arc<DaemonClient>,
    msg_tx: mpsc::UnboundedSender<Msg>,
    ordered: mpsc::UnboundedSender<Ordered>,
}

fn failure(client: &DaemonClient, msg_tx: &mpsc::UnboundedSender<Msg>, error: &anyhow::Error) {
    // A dead connection is reported by the event stream, not once per request.
    if client.is_connected() {
        let _ = msg_tx.send(Msg::Failed(format!("{error:#}")));
    }
}

impl Io {
    pub fn new(client: Arc<DaemonClient>, msg_tx: mpsc::UnboundedSender<Msg>) -> Self {
        let (ordered, mut rx) = mpsc::unbounded_channel::<Ordered>();
        {
            let client = client.clone();
            let msg_tx = msg_tx.clone();
            tokio::spawn(async move {
                while let Some(op) = rx.recv().await {
                    let result = match op {
                        Ordered::Raw(id, data) => client.send_raw(&id, data).await.map(|_| ()),
                        Ordered::Input(id, text) => client.send_input(&id, &text).await.map(|_| ()),
                        Ordered::Resize(id, cols, rows) => client.resize(&id, cols, rows).await,
                        Ordered::Focus(focused) => client.set_focus(focused).await,
                    };
                    if let Err(err) = result {
                        failure(&client, &msg_tx, &err);
                    }
                }
            });
        }
        Self {
            client,
            msg_tx,
            ordered,
        }
    }

    pub fn client(&self) -> &Arc<DaemonClient> {
        &self.client
    }

    /// Run an effect that talks to the daemon (`Quit` and `Bell` belong to the caller).
    pub fn run(&self, effect: Effect) {
        let client = self.client.clone();
        let tx = self.msg_tx.clone();
        match effect {
            Effect::SendRaw(id, data) => {
                let _ = self.ordered.send(Ordered::Raw(id, data));
            }
            Effect::SendInput(id, text) => {
                let _ = self.ordered.send(Ordered::Input(id, text));
            }
            Effect::Resize(id, cols, rows) => {
                let _ = self.ordered.send(Ordered::Resize(id, cols, rows));
            }
            Effect::SetFocus(focused) => {
                let _ = self.ordered.send(Ordered::Focus(focused));
            }
            Effect::Refresh => {
                tokio::spawn(async move { refresh(&client, &tx, true).await });
            }
            Effect::CreateAgent(payload) => {
                tokio::spawn(async move {
                    let result = client
                        .create_agent(payload)
                        .await
                        .map(Box::new)
                        .map_err(|e| format!("{e:#}"));
                    let _ = tx.send(Msg::Created(result));
                });
            }
            Effect::CreateAgentWithProfile {
                project_dir,
                config_dir,
                payload,
            } => {
                tokio::spawn(async move {
                    // The mapping first: the daemon reads it when it spawns the agent.
                    match client
                        .set_project_profile(&project_dir, Some(config_dir))
                        .await
                    {
                        Ok(config) => {
                            let _ = tx.send(Msg::Config(Box::new(config)));
                        }
                        Err(err) => {
                            let _ = tx.send(Msg::Created(Err(format!("{err:#}"))));
                            return;
                        }
                    }
                    let result = client
                        .create_agent(payload)
                        .await
                        .map(Box::new)
                        .map_err(|e| format!("{e:#}"));
                    let _ = tx.send(Msg::Created(result));
                });
            }
            Effect::LoadProfiles => {
                tokio::spawn(async move {
                    match client.list_claude_profiles().await {
                        Ok(profiles) => {
                            let _ = tx.send(Msg::Profiles(profiles));
                        }
                        Err(err) => tracing::debug!(%err, "listing Claude profiles failed"),
                    }
                });
            }
            Effect::LoadModels(provider) => {
                tokio::spawn(async move {
                    match client.list_provider_models(provider).await {
                        Ok(models) => {
                            let _ = tx.send(Msg::Models { provider, models });
                        }
                        Err(err) => tracing::debug!(%err, "listing models failed"),
                    }
                });
            }
            Effect::LoadSessions { provider, force } => {
                tokio::spawn(async move {
                    let options = as_core::sessions::ListSessionsOptions {
                        provider,
                        limit: Some(SESSION_LIST_LIMIT),
                        include_hidden: true,
                        force_refresh: force,
                        ..Default::default()
                    };
                    let result = client
                        .list_sessions(options)
                        .await
                        .map_err(|e| format!("{e:#}"));
                    let _ = tx.send(Msg::Sessions { provider, result });
                });
            }
            Effect::HideSession(session_id, hidden) => {
                tokio::spawn(async move {
                    match client.hide_session(&session_id, hidden).await {
                        Ok(config) => {
                            let _ = tx.send(Msg::Config(Box::new(config)));
                        }
                        Err(err) => failure(&client, &tx, &err),
                    }
                });
            }
            Effect::SetProjectProfile {
                project_dir,
                config_dir,
            } => {
                tokio::spawn(async move {
                    match client.set_project_profile(&project_dir, config_dir).await {
                        Ok(config) => {
                            let _ = tx.send(Msg::Config(Box::new(config)));
                        }
                        Err(err) => failure(&client, &tx, &err),
                    }
                });
            }
            Effect::Kill(id) => {
                tokio::spawn(async move {
                    match client.kill_agent(&id).await {
                        Ok(true) => {}
                        Ok(false) => {
                            let _ = tx.send(Msg::Failed("unknown agent".into()));
                        }
                        Err(err) => failure(&client, &tx, &err),
                    }
                    refresh(&client, &tx, false).await;
                });
            }
            Effect::Restart(id) => {
                tokio::spawn(async move {
                    if let Err(err) = client.restart_agent(&id).await {
                        failure(&client, &tx, &err);
                    }
                    refresh(&client, &tx, false).await;
                });
            }
            Effect::Remove(id) => {
                tokio::spawn(async move {
                    if let Err(err) = client.remove_agent(&id).await {
                        failure(&client, &tx, &err);
                    }
                    refresh(&client, &tx, false).await;
                });
            }
            Effect::Rename(id, name) => {
                tokio::spawn(async move {
                    if let Err(err) = client.rename_agent(&id, &name).await {
                        failure(&client, &tx, &err);
                    }
                    refresh(&client, &tx, false).await;
                });
            }
            Effect::ToggleYolo(id, yolo) => {
                tokio::spawn(async move {
                    if let Err(err) = client.toggle_yolo(&id, yolo).await {
                        failure(&client, &tx, &err);
                    }
                    refresh(&client, &tx, false).await;
                });
            }
            Effect::SetGlobalYolo(yolo) => {
                tokio::spawn(async move {
                    match client
                        .update_config(serde_json::json!({ "globalYolo": yolo }))
                        .await
                    {
                        Ok(config) => {
                            let _ = tx.send(Msg::Config(Box::new(config)));
                        }
                        Err(err) => failure(&client, &tx, &err),
                    }
                    if let Err(err) = client.set_global_yolo(yolo).await {
                        failure(&client, &tx, &err);
                    }
                    refresh(&client, &tx, false).await;
                });
            }
            Effect::Subscribe(id) => {
                tokio::spawn(async move {
                    let result = client.subscribe(&id).await.map_err(|e| format!("{e:#}"));
                    let _ = tx.send(Msg::Subscribed {
                        agent_id: id,
                        result,
                    });
                });
            }
            Effect::Unsubscribe(id) => {
                tokio::spawn(async move {
                    let _ = client.unsubscribe(&id).await;
                });
            }
            Effect::ShutdownAndQuit => {
                tokio::spawn(async move {
                    let result = client.shutdown(true).await.map_err(|e| format!("{e:#}"));
                    let _ = tx.send(Msg::ShutdownDone(result));
                });
            }
            Effect::SetGridColumns(columns) => {
                tokio::spawn(async move {
                    let value = serde_json::to_value(columns).unwrap_or(serde_json::Value::Null);
                    match client
                        .update_config(serde_json::json!({ "gridColumns": value }))
                        .await
                    {
                        Ok(config) => {
                            let _ = tx.send(Msg::Config(Box::new(config)));
                        }
                        Err(err) => failure(&client, &tx, &err),
                    }
                });
            }
            // Handled by the runner (needs the terminal).
            Effect::Quit | Effect::Bell | Effect::Suspend(_) => {}
        }
    }
}

/// ListAgents (+ GetConfig when `with_config`) -> messages.
async fn refresh(client: &DaemonClient, tx: &mpsc::UnboundedSender<Msg>, with_config: bool) {
    match client.list_agents().await {
        Ok(agents) => {
            let _ = tx.send(Msg::Agents(agents));
        }
        Err(err) => failure(client, tx, &err),
    }
    if with_config {
        match client.get_config().await {
            Ok(config) => {
                let _ = tx.send(Msg::Config(Box::new(config)));
            }
            Err(err) => failure(client, tx, &err),
        }
    }
}
