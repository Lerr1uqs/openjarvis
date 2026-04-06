//! Manual Feishu attachment probe that uploads one local image and sends it to one chat.

use anyhow::{Context, Result, bail};
use clap::Parser;
use openjarvis::{
    config::{AppConfig, FeishuConfig},
    logging,
};
use reqwest::{
    Client,
    multipart::{Form, Part},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
};
use tracing::info;
use uuid::Uuid;

/// Command-line options for the manual Feishu attachment probe.
#[derive(Debug, Clone, Parser)]
#[command(name = "feishu_attachment_probe")]
struct FeishuAttachmentProbeCli {
    /// Chat id that will receive the probe message.
    #[arg(long)]
    chat_id: String,
    /// Local image file path that will be uploaded to Feishu.
    #[arg(long)]
    image: PathBuf,
    /// Optional text sent before the image.
    #[arg(long)]
    text: Option<String>,
    /// Reuse the same UUID for text and image sends to reproduce idempotency issues.
    #[arg(long, default_value_t = false)]
    same_uuid: bool,
    /// Skip the text send and dispatch only the image message.
    #[arg(long, default_value_t = false)]
    image_only: bool,
    /// Optional config path override. Falls back to OPENJARVIS_CONFIG or config.yaml.
    #[arg(long)]
    config: Option<PathBuf>,
}

/// Run the manual Feishu attachment probe end to end.
///
/// # 示例
/// ```rust,no_run
/// # async fn demo() -> anyhow::Result<()> {
/// use clap::Parser;
///
/// let _cli = clap::Parser::parse_from([
///     "feishu_attachment_probe",
///     "--chat-id",
///     "oc_demo",
///     "--image",
///     "/tmp/demo.png",
/// ]);
/// # Ok(())
/// # }
/// ```
async fn run_probe(cli: &FeishuAttachmentProbeCli) -> Result<()> {
    let config = load_config(cli.config.as_deref())?;
    let feishu = config.channel_config().feishu_config().clone();
    ensure_feishu_delivery_config(&feishu)?;
    if !cli.image.exists() || !cli.image.is_file() {
        bail!("probe image does not exist: {}", cli.image.display());
    }
    if cli.chat_id.trim().is_empty() {
        bail!("`--chat-id` must not be blank");
    }

    let client = Client::new();
    let tenant_access_token = request_tenant_access_token(&client, &feishu).await?;
    let upload = upload_image(&client, &feishu, &tenant_access_token, &cli.image).await?;
    let image_key = upload.image_key;
    let base_uuid = Uuid::new_v4();
    let text_uuid = base_uuid;
    let image_uuid = if cli.same_uuid {
        base_uuid
    } else {
        Uuid::new_v4()
    };

    info!(
        chat_id = cli.chat_id,
        image = %cli.image.display(),
        same_uuid = cli.same_uuid,
        text_uuid = %text_uuid,
        image_uuid = %image_uuid,
        "starting feishu attachment probe"
    );

    if !cli.image_only {
        let text = cli
            .text
            .clone()
            .unwrap_or_else(|| "[openjarvis][DEBUG] attachment probe text".to_string());
        let response = send_message(
            &client,
            &feishu,
            &tenant_access_token,
            &cli.chat_id,
            "text",
            json!({ "text": text }),
            text_uuid,
        )
        .await?;
        println!(
            "text send response:\n{}",
            serde_json::to_string_pretty(&response).context("failed to format text response")?
        );
    }

    let response = send_message(
        &client,
        &feishu,
        &tenant_access_token,
        &cli.chat_id,
        "image",
        json!({ "image_key": image_key }),
        image_uuid,
    )
    .await?;
    println!(
        "image send response:\n{}",
        serde_json::to_string_pretty(&response).context("failed to format image response")?
    );

    Ok(())
}

/// Load app config from one explicit path or from the default config resolution rules.
fn load_config(config_path: Option<&Path>) -> Result<AppConfig> {
    match config_path {
        Some(path) => AppConfig::from_path(path),
        None => AppConfig::load(),
    }
}

/// Validate the Feishu config fields required for real delivery.
fn ensure_feishu_delivery_config(config: &FeishuConfig) -> Result<()> {
    if config.dry_run {
        bail!("feishu_attachment_probe requires feishu.dry_run=false");
    }
    if config.app_id.trim().is_empty() || config.app_secret.trim().is_empty() {
        bail!("feishu app_id/app_secret are required for the probe");
    }
    if config.open_base_url.trim().is_empty() {
        bail!("feishu open_base_url is required for the probe");
    }

    Ok(())
}

/// Request one Feishu tenant access token for the current application credentials.
async fn request_tenant_access_token(client: &Client, config: &FeishuConfig) -> Result<String> {
    let endpoint = format!(
        "{}/open-apis/auth/v3/tenant_access_token/internal",
        config.open_base_url.trim_end_matches('/')
    );
    info!(endpoint = %endpoint, "requesting feishu tenant access token for probe");
    let response = client
        .post(endpoint)
        .json(&json!({
            "app_id": config.app_id,
            "app_secret": config.app_secret,
        }))
        .send()
        .await
        .context("failed to request feishu tenant access token")?;
    let status = response.status();
    let body = response
        .text()
        .await
        .context("failed to read feishu tenant access token response")?;
    if !status.is_success() {
        bail!("feishu tenant access token request failed with status {status}: {body}");
    }

    let payload: TenantAccessTokenResponse = serde_json::from_str(&body)
        .context("failed to decode feishu tenant access token response")?;
    if payload.code != 0 {
        bail!(
            "feishu tenant access token request returned code {} with message {}",
            payload.code,
            payload.msg
        );
    }

    Ok(payload.tenant_access_token)
}

/// Upload one local image to Feishu and return the resolved `image_key`.
async fn upload_image(
    client: &Client,
    config: &FeishuConfig,
    tenant_access_token: &str,
    image_path: &Path,
) -> Result<ImageUploadResult> {
    let endpoint = format!(
        "{}/open-apis/im/v1/images",
        config.open_base_url.trim_end_matches('/')
    );
    let image_bytes = fs::read(image_path)
        .with_context(|| format!("failed to read probe image {}", image_path.display()))?;
    let file_name = image_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("openjarvis-probe-image");
    let image_part = Part::bytes(image_bytes).file_name(file_name.to_string());
    let image_part = if let Some(mime_type) = guess_image_mime_type(image_path) {
        image_part
            .mime_str(mime_type)
            .context("failed to build multipart body for probe image upload")?
    } else {
        image_part
    };
    let form = Form::new()
        .text("image_type", "message")
        .part("image", image_part);
    info!(
        endpoint = %endpoint,
        image = %image_path.display(),
        "uploading probe image to feishu"
    );
    let response = client
        .post(endpoint)
        .bearer_auth(tenant_access_token)
        .multipart(form)
        .send()
        .await
        .context("failed to upload probe image to feishu")?;
    let status = response.status();
    let body = response
        .text()
        .await
        .context("failed to read feishu image upload response")?;
    if !status.is_success() {
        bail!("feishu image upload failed with status {status}: {body}");
    }

    let payload: ImageUploadResponse =
        serde_json::from_str(&body).context("failed to decode feishu image upload response")?;
    if payload.code != 0 {
        bail!(
            "feishu image upload returned code {} with message {}",
            payload.code,
            payload.msg
        );
    }

    let Some(data) = payload.data else {
        bail!("feishu image upload response did not include image_key");
    };
    println!(
        "image upload response:\n{}",
        serde_json::to_string_pretty(&json!({
            "code": payload.code,
            "msg": payload.msg,
            "image_key": data.image_key,
        }))
        .context("failed to format image upload response")?
    );

    Ok(ImageUploadResult {
        image_key: data.image_key,
    })
}

/// Send one text or image message to the target Feishu chat and return the full API response.
async fn send_message(
    client: &Client,
    config: &FeishuConfig,
    tenant_access_token: &str,
    chat_id: &str,
    msg_type: &str,
    content: Value,
    delivery_uuid: Uuid,
) -> Result<Value> {
    let endpoint = format!(
        "{}/open-apis/im/v1/messages",
        config.open_base_url.trim_end_matches('/')
    );
    info!(
        endpoint = %endpoint,
        chat_id,
        msg_type,
        delivery_uuid = %delivery_uuid,
        "sending feishu probe message"
    );
    let response = client
        .post(endpoint)
        .bearer_auth(tenant_access_token)
        .query(&[("receive_id_type", "chat_id")])
        .json(&json!({
            "receive_id": chat_id,
            "msg_type": msg_type,
            "content": content.to_string(),
            "uuid": delivery_uuid.to_string(),
        }))
        .send()
        .await
        .with_context(|| format!("failed to send feishu `{msg_type}` probe message"))?;
    let status = response.status();
    let body = response
        .text()
        .await
        .with_context(|| format!("failed to read feishu `{msg_type}` probe response"))?;
    if !status.is_success() {
        bail!("feishu `{msg_type}` send failed with status {status}: {body}");
    }

    let payload = serde_json::from_str::<Value>(&body)
        .with_context(|| format!("failed to decode feishu `{msg_type}` response"))?;
    if payload["code"].as_i64().unwrap_or(-1) != 0 {
        bail!(
            "feishu `{msg_type}` send returned code {} with message {}",
            payload["code"],
            payload["msg"]
        );
    }

    Ok(payload)
}

/// Guess the MIME type for one local image file by extension.
fn guess_image_mime_type(path: &Path) -> Option<&'static str> {
    match path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.to_ascii_lowercase())
        .as_deref()
    {
        Some("png") => Some("image/png"),
        Some("jpg") | Some("jpeg") => Some("image/jpeg"),
        Some("gif") => Some("image/gif"),
        Some("webp") => Some("image/webp"),
        Some("bmp") => Some("image/bmp"),
        _ => None,
    }
}

#[derive(Debug, Deserialize)]
struct TenantAccessTokenResponse {
    code: i32,
    msg: String,
    tenant_access_token: String,
}

#[derive(Debug, Deserialize)]
struct ImageUploadResponse {
    code: i32,
    msg: String,
    data: Option<ImageUploadResponseData>,
}

#[derive(Debug, Deserialize)]
struct ImageUploadResponseData {
    image_key: String,
}

struct ImageUploadResult {
    image_key: String,
}

/// Main entrypoint for the manual Feishu attachment probe.
#[tokio::main]
async fn main() -> Result<()> {
    let cli = FeishuAttachmentProbeCli::parse();
    let _logging_guards = logging::init_tracing_from_default_config().ok();
    run_probe(&cli).await
}
