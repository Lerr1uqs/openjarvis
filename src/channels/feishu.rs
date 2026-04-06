//! Feishu channel implementation backed by long-connection sidecar ingestion and HTTP replies.

use crate::channels::{Channel, ChannelRegistration};
use crate::config::FeishuConfig;
use crate::model::{
    IncomingMessage, OutgoingAttachment, OutgoingAttachmentKind, OutgoingMessage, ReplyTarget,
};
use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use chrono::{Duration, Utc};
use reqwest::{
    Client,
    multipart::{Form, Part},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{fs, path::Path, process::Stdio, sync::Arc, time::Instant};
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

    /// Convert one Feishu long-connection payload into the unified incoming message model.
    ///
    /// OpenJarvis uses one channel-level conversation identifier as `external_thread_id`.
    /// For Feishu, `chat_id` identifies the whole chat container, while Feishu `thread_id`
    /// only points to one topic thread inside that chat and must not be used to split the
    /// OpenJarvis conversation. The raw Feishu `thread_id` is preserved in `metadata`.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::{
    ///     channels::feishu::{FeishuChannel, FeishuLongConnectionPayload},
    ///     config::FeishuConfig,
    /// };
    /// use serde_json::json;
    ///
    /// let channel = FeishuChannel::new(FeishuConfig::default());
    /// let incoming = channel.parse_long_connection_incoming(
    ///     serde_json::from_value::<FeishuLongConnectionPayload>(json!({
    ///         "event_id": "evt_ws_demo",
    ///         "sender_open_id": "ou_demo",
    ///         "sender_type": "user",
    ///         "tenant_key": "tenant_demo",
    ///         "message_id": "om_demo",
    ///         "chat_id": "oc_demo",
    ///         "thread_id": "omt_demo",
    ///         "chat_type": "group",
    ///         "message_type": "text",
    ///         "content": "{\"text\":\"hello\"}"
    ///     }))
    ///     .expect("payload should deserialize"),
    /// );
    ///
    /// assert_eq!(incoming.external_thread_id.as_deref(), Some("oc_demo"));
    /// assert_eq!(incoming.metadata["feishu_thread_id"], "omt_demo");
    /// ```
    pub fn parse_long_connection_incoming(
        &self,
        payload: FeishuLongConnectionPayload,
    ) -> IncomingMessage {
        let content = extract_text_message(&payload.message_type, &payload.content);
        let message_id = payload.message_id.clone();
        let chat_id = payload.chat_id.clone();
        let raw_thread_id = payload.thread_id.clone();
        // Feishu `chat_id` identifies the whole conversation container. Its `thread_id`
        // only identifies one topic thread inside the same chat, so OpenJarvis should use
        // `chat_id` as the stable external thread identity.
        let external_thread_id = Some(chat_id.clone()).filter(|value| !value.trim().is_empty());

        debug!(
            message_id,
            chat_id,
            message_type = %payload.message_type,
            content = %content,
            has_thread_id = raw_thread_id.is_some(),
            external_thread_id_is_none = external_thread_id.is_none(),
            raw_feishu_thread_id = ?raw_thread_id,
            openjarvis_external_thread_id = ?external_thread_id,
            "parsed feishu long-connection message"
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
                "feishu_thread_id": raw_thread_id,
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

        if !message.content.trim().is_empty() {
            self.send_text_message(&message).await?;
        }

        for attachment in &message.attachments {
            self.send_attachment(&message, attachment).await?;
        }

        if message.content.trim().is_empty() && message.attachments.is_empty() {
            info!(
                message_id = %message.id,
                receive_id = message.target.receive_id,
                "feishu outgoing message skipped because text and attachments are empty"
            );
        }

        Ok(())
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

        self.ensure_delivery_credentials()?;
        self.send_message_payload(message, "text", json!({ "text": message.content }))
            .await
    }

    async fn send_attachment(
        &self,
        message: &OutgoingMessage,
        attachment: &OutgoingAttachment,
    ) -> Result<()> {
        match attachment.kind {
            OutgoingAttachmentKind::Image => self.send_image_attachment(message, attachment).await,
        }
    }

    async fn send_image_attachment(
        &self,
        message: &OutgoingMessage,
        attachment: &OutgoingAttachment,
    ) -> Result<()> {
        if self.config.dry_run {
            info!(
                message_id = %message.id,
                receive_id = message.target.receive_id,
                path = attachment.path,
                "feishu dry_run enabled, outgoing image skipped"
            );
            return Ok(());
        }

        self.ensure_delivery_credentials()?;
        let image_key = self.upload_image_attachment(attachment).await?;
        self.send_message_payload(message, "image", json!({ "image_key": image_key }))
            .await
    }

    async fn send_message_payload(
        &self,
        message: &OutgoingMessage,
        msg_type: &str,
        content: Value,
    ) -> Result<()> {
        let access_token = self.get_tenant_access_token().await?;
        let endpoint = format!(
            "{}/open-apis/im/v1/messages",
            self.config.open_base_url.trim_end_matches('/')
        );
        let started_at = Instant::now();
        let delivery_uuid = Uuid::new_v4();
        debug!(
            endpoint = %endpoint,
            message_id = %message.id,
            delivery_uuid = %delivery_uuid,
            receive_id = %message.target.receive_id,
            receive_id_type = %message.target.receive_id_type,
            msg_type,
            "starting feishu send-message request"
        );
        let response = self
            .http_client
            .post(endpoint)
            .bearer_auth(access_token)
            .query(&[("receive_id_type", message.target.receive_id_type.clone())])
            .json(&json!({
                "receive_id": message.target.receive_id,
                "msg_type": msg_type,
                "content": content.to_string(),
                "uuid": delivery_uuid.to_string(),
            }))
            .send()
            .await
            .context("failed to send feishu message")?;

        let status = response.status();
        let body = response
            .text()
            .await
            .context("failed to read feishu send-message response")?;
        debug!(
            message_id = %message.id,
            delivery_uuid = %delivery_uuid,
            receive_id = %message.target.receive_id,
            msg_type,
            status = %status,
            elapsed_ms = started_at.elapsed().as_millis() as u64,
            body_len = body.len(),
            "completed feishu send-message request"
        );
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

    async fn upload_image_attachment(&self, attachment: &OutgoingAttachment) -> Result<String> {
        let image_bytes = fs::read(&attachment.path)
            .with_context(|| format!("failed to read outgoing image {}", attachment.path))?;
        let file_name = Path::new(&attachment.path)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("openjarvis-image");
        let endpoint = format!(
            "{}/open-apis/im/v1/images",
            self.config.open_base_url.trim_end_matches('/')
        );
        let image_part = Part::bytes(image_bytes).file_name(file_name.to_string());
        let image_part = if let Some(mime_type) = resolve_image_mime_type(attachment) {
            image_part
                .mime_str(&mime_type)
                .context("failed to build feishu image upload multipart body")?
        } else {
            image_part
        };
        let form = Form::new()
            .text("image_type", "message")
            .part("image", image_part);
        let access_token = self.get_tenant_access_token().await?;
        let started_at = Instant::now();
        debug!(
            endpoint = %endpoint,
            path = attachment.path,
            "starting feishu image upload request"
        );
        let response = self
            .http_client
            .post(endpoint)
            .bearer_auth(access_token)
            .multipart(form)
            .send()
            .await
            .context("failed to upload feishu image")?;

        let status = response.status();
        let body = response
            .text()
            .await
            .context("failed to read feishu image upload response")?;
        debug!(
            path = attachment.path,
            status = %status,
            elapsed_ms = started_at.elapsed().as_millis() as u64,
            body_len = body.len(),
            "completed feishu image upload request"
        );
        if !status.is_success() {
            bail!("feishu image upload request failed with status {status}: {body}");
        }

        let payload: FeishuImageUploadResponse =
            serde_json::from_str(&body).context("failed to decode feishu image upload response")?;
        if payload.code != 0 {
            bail!(
                "feishu image upload returned code {} with message {}",
                payload.code,
                payload.msg
            );
        }

        payload
            .data
            .map(|data| data.image_key)
            .filter(|image_key| !image_key.trim().is_empty())
            .context("feishu image upload response did not contain image_key")
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
        let started_at = Instant::now();
        debug!(
            endpoint = %endpoint,
            message_id,
            emoji_type,
            "starting feishu reaction request"
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
        debug!(
            message_id,
            emoji_type,
            status = %status,
            elapsed_ms = started_at.elapsed().as_millis() as u64,
            body_len = body.len(),
            "completed feishu reaction request"
        );
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
                    debug!(
                        expires_at = %cached_token.expires_at,
                        remaining_seconds = (cached_token.expires_at - now).num_seconds(),
                        "reusing cached feishu tenant_access_token"
                    );
                    return Ok(cached_token.value.clone());
                }
            }
        }

        let endpoint = format!(
            "{}/open-apis/auth/v3/tenant_access_token/internal",
            self.config.open_base_url.trim_end_matches('/')
        );
        let started_at = Instant::now();
        debug!(
            endpoint = %endpoint,
            app_id = %self.config.app_id,
            "starting feishu tenant_access_token request"
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
        debug!(
            status = %status,
            elapsed_ms = started_at.elapsed().as_millis() as u64,
            body_len = body.len(),
            "completed feishu tenant_access_token request"
        );
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
        debug!(
            expires_at = %expires_at,
            ttl_seconds = payload.expire,
            "cached feishu tenant_access_token"
        );

        Ok(payload.tenant_access_token)
    }

    fn ensure_delivery_credentials(&self) -> Result<()> {
        if self.config.app_id.trim().is_empty() || self.config.app_secret.trim().is_empty() {
            bail!("feishu app_id/app_secret are required when feishu.dry_run=false");
        }

        Ok(())
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
struct FeishuImageUploadResponse {
    code: i32,
    msg: String,
    data: Option<FeishuImageUploadResponseData>,
}

#[derive(Debug, Deserialize)]
struct FeishuImageUploadResponseData {
    image_key: String,
}

#[derive(Debug, Deserialize)]
struct FeishuBaseResponse {
    code: i32,
    msg: String,
}

fn resolve_image_mime_type(attachment: &OutgoingAttachment) -> Option<String> {
    if let Some(mime_type) = attachment.mime_type.as_ref() {
        return Some(mime_type.clone());
    }

    match Path::new(&attachment.path)
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.to_ascii_lowercase())
        .as_deref()
    {
        Some("png") => Some("image/png".to_string()),
        Some("jpg") | Some("jpeg") => Some("image/jpeg".to_string()),
        Some("gif") => Some("image/gif".to_string()),
        Some("webp") => Some("image/webp".to_string()),
        Some("bmp") => Some("image/bmp".to_string()),
        _ => None,
    }
}
