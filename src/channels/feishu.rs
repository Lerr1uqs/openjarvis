//! Feishu channel implementation backed by long-connection sidecar ingestion and HTTP replies.

use crate::channels::{Channel, ChannelRegistration};
use crate::config::FeishuConfig;
use crate::model::{IncomingMessage, OutgoingMessage, ReplyTarget};
use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use chrono::{Duration, Utc};
use reqwest::Client;
use serde::Deserialize;
use serde_json::json;
use std::{path::Path, process::Stdio, sync::Arc};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::Command,
    sync::{Mutex, mpsc},
};
use tracing::{debug, info, warn};
use uuid::Uuid;

const FEISHU_DEBUG_REACTION_EMOJI_TYPE: &str = "Typing";

pub struct FeishuChannel {
    config: FeishuConfig,
    http_client: Client,
    cached_token: Mutex<Option<CachedTenantToken>>,
}

impl FeishuChannel {
    /// Create a Feishu channel with its HTTP client and token cache initialized.
    pub fn new(config: FeishuConfig) -> Self {
        Self {
            config,
            http_client: Client::new(),
            cached_token: Mutex::new(None),
        }
    }

    pub fn parse_long_connection_incoming(
        &self,
        payload: FeishuLongConnectionPayload,
    ) -> IncomingMessage {
        let content = extract_text_message(&payload.message_type, &payload.content);
        let message_id = payload.message_id.clone();
        let chat_id = payload.chat_id.clone();
        let raw_thread_id = payload.thread_id.clone();
        let external_thread_id = raw_thread_id
            .clone()
            .filter(|value| !value.trim().is_empty());

        debug!(
            message_id,
            chat_id,
            has_thread_id = raw_thread_id.is_some(),
            raw_thread_id = ?raw_thread_id,
            resolved_external_thread_id = ?external_thread_id,
            "parsed feishu long-connection external_thread_id"
        );

        IncomingMessage {
            id: Uuid::new_v4(),
            external_message_id: Some(message_id.clone()),
            channel: self.name().to_string(),
            user_id: payload.sender_open_id,
            user_name: None,
            content,
            external_thread_id,
            received_at: Utc::now(),
            metadata: json!({
                "event_id": payload.event_id,
                "event_type": "im.message.receive_v1",
                "chat_id": chat_id,
                "chat_type": payload.chat_type,
                "message_id": message_id,
                "message_type": payload.message_type,
                "tenant_key": payload.tenant_key,
                "source": "feishu_long_connection",
            }),
            attachments: Vec::new(),
            reply_target: ReplyTarget {
                receive_id: payload.chat_id,
                receive_id_type: "chat_id".to_string(),
            },
        }
    }

    async fn start_long_connection(
        self: Arc<Self>,
        registration: ChannelRegistration,
    ) -> Result<()> {
        // Start both the outgoing delivery loop and the incoming sidecar reader.
        self.spawn_outgoing_loop(registration.outgoing_rx);
        self.spawn_sidecar(registration.incoming_tx).await
    }

    fn spawn_outgoing_loop(self: &Arc<Self>, mut outgoing_rx: mpsc::Receiver<OutgoingMessage>) {
        // Drain router-originated outgoing messages and forward them to Feishu.
        let channel = Arc::clone(self);
        tokio::spawn(async move {
            while let Some(message) = outgoing_rx.recv().await {
                if let Err(error) = channel.deliver_outgoing(message).await {
                    warn!(error = %error, "failed to deliver feishu outgoing message");
                }
            }
        });
    }

    async fn spawn_sidecar(
        self: &Arc<Self>,
        incoming_tx: mpsc::Sender<IncomingMessage>,
    ) -> Result<()> {
        // Spawn the Node sidecar and forward parsed events into the router queue.
        if !self.config.auto_start_sidecar {
            bail!("feishu long_connection requires auto_start_sidecar=true in current runtime");
        }
        if self.config.app_id.trim().is_empty() || self.config.app_secret.trim().is_empty() {
            bail!("feishu app_id/app_secret are required for long_connection mode");
        }

        let script_path = Path::new(&self.config.sidecar_script);
        if !script_path.exists() {
            bail!(
                "feishu sidecar script not found at {}",
                script_path.display()
            );
        }

        let mut child = Command::new(&self.config.node_bin)
            .arg(script_path)
            .env("FEISHU_APP_ID", &self.config.app_id)
            .env("FEISHU_APP_SECRET", &self.config.app_secret)
            .env("FEISHU_OPEN_BASE_URL", &self.config.open_base_url)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .context("failed to spawn feishu sidecar")?;

        let stdout = child
            .stdout
            .take()
            .context("failed to capture feishu sidecar stdout")?;
        let channel = Arc::clone(self);

        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            loop {
                match lines.next_line().await {
                    Ok(Some(line)) => {
                        let line = line.trim().to_string();
                        if line.is_empty() {
                            continue;
                        }
                        if !line.starts_with('{') {
                            continue;
                        }

                        info!(
                            raw_payload = %line,
                            "received raw feishu long-connection payload"
                        );

                        match serde_json::from_str::<FeishuLongConnectionPayload>(&line) {
                            Ok(payload) => {
                                if payload.sender_type != "user" {
                                    warn!(
                                        sender_type = payload.sender_type,
                                        "non-user feishu long-connection message ignored"
                                    );
                                    continue;
                                }

                                let incoming = channel.parse_long_connection_incoming(payload);
                                if let Err(error) = incoming_tx.send(incoming).await {
                                    warn!(
                                        error = %error,
                                        "failed to forward feishu message into router channel"
                                    );
                                    break;
                                }
                            }
                            Err(error) => {
                                warn!(
                                    error = %error,
                                    line,
                                    "failed to parse feishu sidecar payload"
                                );
                            }
                        }
                    }
                    Ok(None) => break,
                    Err(error) => {
                        warn!(error = %error, "failed to read feishu sidecar stdout");
                        break;
                    }
                }
            }

            match child.wait().await {
                Ok(status) => warn!(status = %status, "feishu sidecar exited"),
                Err(error) => warn!(error = %error, "failed to wait for feishu sidecar"),
            }
        });

        info!(
            script = self.config.sidecar_script,
            "spawned feishu long-connection sidecar"
        );
        Ok(())
    }

    async fn deliver_outgoing(&self, message: OutgoingMessage) -> Result<()> {
        // Add the debug reaction first and then send the actual text reply.
        if let Some(reply_to_message_id) = message.reply_to_message_id.as_deref() {
            if let Err(error) = self
                .add_reaction(reply_to_message_id, FEISHU_DEBUG_REACTION_EMOJI_TYPE)
                .await
            {
                warn!(
                    error = %error,
                    message_id = reply_to_message_id,
                    emoji_type = FEISHU_DEBUG_REACTION_EMOJI_TYPE,
                    "failed to add feishu reaction before reply"
                );
            }
        }

        self.send_text_message(&message).await
    }

    async fn send_text_message(&self, message: &OutgoingMessage) -> Result<()> {
        // Call the Feishu send-message API unless the channel is running in dry-run mode.
        if self.config.dry_run {
            info!(
                receive_id = message.target.receive_id,
                content = message.content,
                "feishu dry_run enabled, outgoing message skipped"
            );
            return Ok(());
        }

        if self.config.app_id.trim().is_empty() || self.config.app_secret.trim().is_empty() {
            bail!("feishu app_id/app_secret are required when feishu.dry_run=false");
        }

        let access_token = self.get_tenant_access_token().await?;
        let endpoint = format!(
            "{}/open-apis/im/v1/messages",
            self.config.open_base_url.trim_end_matches('/')
        );
        let response = self
            .http_client
            .post(endpoint)
            .bearer_auth(access_token)
            .query(&[("receive_id_type", message.target.receive_id_type.clone())])
            .json(&json!({
                "receive_id": message.target.receive_id,
                "msg_type": "text",
                "content": json!({ "text": message.content }).to_string(),
                "uuid": message.id.to_string(),
            }))
            .send()
            .await
            .context("failed to send feishu message")?;

        let status = response.status();
        let body = response
            .text()
            .await
            .context("failed to read feishu send-message response")?;
        if !status.is_success() {
            bail!("feishu send-message request failed with status {status}: {body}");
        }

        let payload: FeishuSendResponse =
            serde_json::from_str(&body).context("failed to decode feishu send response")?;
        if payload.code != 0 {
            bail!(
                "feishu send-message returned code {} with message {}",
                payload.code,
                payload.msg
            );
        }

        Ok(())
    }

    async fn add_reaction(&self, message_id: &str, emoji_type: &str) -> Result<()> {
        // Add a Feishu reaction to the source message so the debug loop is visible in chat.
        if self.config.dry_run {
            info!(
                message_id,
                emoji_type, "feishu dry_run enabled, reaction skipped"
            );
            return Ok(());
        }

        if self.config.app_id.trim().is_empty() || self.config.app_secret.trim().is_empty() {
            bail!("feishu app_id/app_secret are required when feishu.dry_run=false");
        }

        let access_token = self.get_tenant_access_token().await?;
        let endpoint = format!(
            "{}/open-apis/im/v1/messages/{}/reactions",
            self.config.open_base_url.trim_end_matches('/'),
            message_id
        );
        let response = self
            .http_client
            .post(endpoint)
            .bearer_auth(access_token)
            .json(&json!({
                "reaction_type": {
                    "emoji_type": emoji_type,
                }
            }))
            .send()
            .await
            .context("failed to add feishu reaction")?;

        let status = response.status();
        let body = response
            .text()
            .await
            .context("failed to read feishu reaction response")?;
        if !status.is_success() {
            bail!("feishu reaction request failed with status {status}: {body}");
        }

        let payload: FeishuBaseResponse =
            serde_json::from_str(&body).context("failed to decode feishu reaction response")?;
        if payload.code != 0 {
            bail!(
                "feishu reaction returned code {} with message {}",
                payload.code,
                payload.msg
            );
        }

        Ok(())
    }

    async fn get_tenant_access_token(&self) -> Result<String> {
        // Reuse a cached tenant token when possible and refresh it otherwise.
        let now = Utc::now();
        {
            let cached_token = self.cached_token.lock().await;
            if let Some(cached_token) = cached_token.as_ref() {
                if cached_token.expires_at > now {
                    return Ok(cached_token.value.clone());
                }
            }
        }

        let endpoint = format!(
            "{}/open-apis/auth/v3/tenant_access_token/internal",
            self.config.open_base_url.trim_end_matches('/')
        );
        let response = self
            .http_client
            .post(endpoint)
            .json(&json!({
                "app_id": self.config.app_id,
                "app_secret": self.config.app_secret,
            }))
            .send()
            .await
            .context("failed to request feishu tenant_access_token")?;

        let status = response.status();
        let body = response
            .text()
            .await
            .context("failed to read feishu tenant_access_token response")?;
        if !status.is_success() {
            bail!("feishu access-token request failed with status {status}: {body}");
        }

        let payload: FeishuTenantAccessTokenResponse = serde_json::from_str(&body)
            .context("failed to decode feishu tenant_access_token response")?;
        if payload.code != 0 {
            bail!(
                "feishu access-token returned code {} with message {}",
                payload.code,
                payload.msg
            );
        }

        let expires_at = now + Duration::seconds(i64::from(payload.expire) - 300);
        let mut cached_token = self.cached_token.lock().await;
        *cached_token = Some(CachedTenantToken {
            value: payload.tenant_access_token.clone(),
            expires_at,
        });

        Ok(payload.tenant_access_token)
    }
}

#[async_trait]
impl Channel for FeishuChannel {
    fn name(&self) -> &'static str {
        "feishu"
    }

    async fn start(self: Arc<Self>, registration: ChannelRegistration) -> Result<()> {
        if self.config.is_long_connection() {
            self.start_long_connection(registration).await
        } else {
            bail!("feishu webhook runtime is not implemented in the mpsc refactor yet");
        }
    }
}

/// Extract user-visible text from a Feishu message payload, falling back to a placeholder for unsupported types.
pub fn extract_text_message(message_type: &str, raw_content: &str) -> String {
    if message_type != "text" {
        return format!("[unsupported feishu message type: {message_type}]");
    }

    serde_json::from_str::<FeishuTextMessageContent>(raw_content)
        .map(|content| content.text)
        .unwrap_or_else(|_| raw_content.to_string())
}

struct CachedTenantToken {
    value: String,
    expires_at: chrono::DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct FeishuLongConnectionPayload {
    event_id: Option<String>,
    sender_open_id: String,
    sender_type: String,
    tenant_key: String,
    message_id: String,
    chat_id: String,
    thread_id: Option<String>,
    chat_type: String,
    message_type: String,
    content: String,
}

#[derive(Debug, Deserialize)]
struct FeishuTextMessageContent {
    text: String,
}

#[derive(Debug, Deserialize)]
struct FeishuTenantAccessTokenResponse {
    code: i32,
    msg: String,
    tenant_access_token: String,
    expire: u32,
}

#[derive(Debug, Deserialize)]
struct FeishuSendResponse {
    code: i32,
    msg: String,
}

#[derive(Debug, Deserialize)]
struct FeishuBaseResponse {
    code: i32,
    msg: String,
}
