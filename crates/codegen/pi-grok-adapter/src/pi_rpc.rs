use anyhow::{Context, Result, anyhow, bail};
use serde_json::Value;
use std::{
    collections::HashMap,
    path::PathBuf,
    process::Stdio,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::Command,
    sync::{mpsc, oneshot},
};

#[derive(Debug, Clone)]
pub struct SpawnConfig {
    pub program: String,
    pub prefix_args: Vec<String>,
    pub cwd: PathBuf,
    pub pi_args: Vec<String>,
}

#[derive(Clone)]
pub struct PiRpc {
    writer: mpsc::UnboundedSender<Value>,
    pending: Arc<Mutex<HashMap<String, oneshot::Sender<Result<Value, String>>>>>,
    next_id: Arc<AtomicU64>,
}

pub struct PiProcess {
    pub rpc: PiRpc,
    pub events: mpsc::UnboundedReceiver<Value>,
}

impl PiRpc {
    pub async fn spawn(config: SpawnConfig) -> Result<PiProcess> {
        let mut command = Command::new(&config.program);
        command
            .args(&config.prefix_args)
            .arg("--mode")
            .arg("rpc")
            .args(&config.pi_args)
            .current_dir(&config.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        let mut child = command.spawn().with_context(|| {
            format!(
                "failed to start Pi RPC process: {} {:?}",
                config.program, config.prefix_args
            )
        })?;
        let mut stdin = child.stdin.take().context("Pi RPC stdin is unavailable")?;
        let stdout = child.stdout.take().context("Pi RPC stdout is unavailable")?;
        let stderr = child.stderr.take().context("Pi RPC stderr is unavailable")?;

        let (writer_tx, mut writer_rx) = mpsc::unbounded_channel::<Value>();
        let (event_tx, event_rx) = mpsc::unbounded_channel::<Value>();
        let pending: Arc<Mutex<HashMap<String, oneshot::Sender<Result<Value, String>>>>> =
            Arc::new(Mutex::new(HashMap::new()));

        tokio::spawn(async move {
            while let Some(value) = writer_rx.recv().await {
                let line = match serde_json::to_vec(&value) {
                    Ok(line) => line,
                    Err(error) => {
                        tracing::error!(%error, "failed to serialize Pi RPC command");
                        continue;
                    }
                };
                if stdin.write_all(&line).await.is_err()
                    || stdin.write_all(b"\n").await.is_err()
                    || stdin.flush().await.is_err()
                {
                    break;
                }
            }
        });

        let pending_stdout = pending.clone();
        let event_stdout = event_tx.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            loop {
                match lines.next_line().await {
                    Ok(Some(line)) => match serde_json::from_str::<Value>(&line) {
                        Ok(value) => {
                            let response_id = value
                                .get("id")
                                .and_then(Value::as_str)
                                .map(ToOwned::to_owned);
                            let is_response = value.get("type").and_then(Value::as_str)
                                == Some("response");
                            if is_response
                                && let Some(id) = response_id
                                && let Some(sender) = pending_stdout
                                    .lock()
                                    .expect("Pi pending map poisoned")
                                    .remove(&id)
                            {
                                let _ = sender.send(Ok(value));
                                continue;
                            }
                            let _ = event_stdout.send(value);
                        }
                        Err(error) => {
                            tracing::warn!(%error, raw = %line, "invalid JSON on Pi RPC stdout");
                            let _ = event_stdout.send(serde_json::json!({
                                "type": "adapter_diagnostic",
                                "message": format!("Invalid Pi RPC JSON: {error}"),
                            }));
                        }
                    },
                    Ok(None) => break,
                    Err(error) => {
                        tracing::warn!(%error, "failed reading Pi RPC stdout");
                        break;
                    }
                }
            }
            fail_pending(&pending_stdout, "Pi RPC stdout closed");
        });

        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                tracing::warn!(target: "pi_rpc", "{line}");
            }
        });

        let pending_exit = pending.clone();
        tokio::spawn(async move {
            let message = match child.wait().await {
                Ok(status) => format!("Pi RPC process exited with {status}"),
                Err(error) => format!("failed waiting for Pi RPC process: {error}"),
            };
            fail_pending(&pending_exit, &message);
            let _ = event_tx.send(serde_json::json!({
                "type": "adapter_process_exit",
                "message": message,
            }));
        });

        Ok(PiProcess {
            rpc: PiRpc {
                writer: writer_tx,
                pending,
                next_id: Arc::new(AtomicU64::new(1)),
            },
            events: event_rx,
        })
    }

    pub async fn request(&self, mut command: Value) -> Result<Value> {
        let command_type = command
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("Pi RPC command is missing its type"))?;
        let timeout = request_timeout(command_type);
        let object = command
            .as_object_mut()
            .ok_or_else(|| anyhow!("Pi RPC command must be a JSON object"))?;
        let id = format!("pi-grok-{}", self.next_id.fetch_add(1, Ordering::Relaxed));
        object.insert("id".to_string(), Value::String(id.clone()));
        let (response_tx, response_rx) = oneshot::channel();
        self.pending
            .lock()
            .expect("Pi pending map poisoned")
            .insert(id.clone(), response_tx);
        if self.writer.send(command).is_err() {
            self.pending
                .lock()
                .expect("Pi pending map poisoned")
                .remove(&id);
            bail!("Pi RPC writer is closed");
        }
        let response = if let Some(timeout) = timeout {
            match tokio::time::timeout(timeout, response_rx).await {
                Ok(response) => response
                    .map_err(|_| anyhow!("Pi RPC response channel closed for {id}"))?
                    .map_err(anyhow::Error::msg)?,
                Err(_) => {
                    self.pending
                        .lock()
                        .expect("Pi pending map poisoned")
                        .remove(&id);
                    bail!("Pi RPC request timed out after {} seconds: {id}", timeout.as_secs());
                }
            }
        } else {
            response_rx
                .await
                .map_err(|_| anyhow!("Pi RPC response channel closed for {id}"))?
                .map_err(anyhow::Error::msg)?
        };
        if response.get("success").and_then(Value::as_bool) == Some(false) {
            let error = response
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("Pi RPC command failed");
            bail!("{error}");
        }
        Ok(response.get("data").cloned().unwrap_or(Value::Null))
    }

    pub fn notify(&self, command: Value) -> Result<()> {
        self.writer
            .send(command)
            .map_err(|_| anyhow!("Pi RPC writer is closed"))
    }
}

fn request_timeout(command_type: &str) -> Option<Duration> {
    match command_type {
        // Pi keeps these requests open until the operation completes; the
        // existing ACP cancel path sends abort_bash/abort without waiting.
        "bash" | "compact" => None,
        _ => Some(Duration::from_secs(300)),
    }
}

fn fail_pending(
    pending: &Arc<Mutex<HashMap<String, oneshot::Sender<Result<Value, String>>>>>,
    message: &str,
) {
    let drained: Vec<_> = pending
        .lock()
        .expect("Pi pending map poisoned")
        .drain()
        .map(|(_, sender)| sender)
        .collect();
    for sender in drained {
        let _ = sender.send(Err(message.to_string()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_long_running_pi_operations_skip_the_default_deadline() {
        assert_eq!(request_timeout("bash"), None);
        assert_eq!(request_timeout("compact"), None);
        assert_eq!(request_timeout("get_state"), Some(Duration::from_secs(300)));
    }
}
