//! Shared JSON-line protocol between Rust browser tools and the Node Playwright sidecar.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One outbound browser sidecar request framed as a single JSON line.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BrowserSidecarRequest {
    pub id: String,
    #[serde(flatten)]
    pub payload: BrowserSidecarRequestPayload,
}

impl BrowserSidecarRequest {
    /// Create one sidecar request with a stable request id.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::agent::tool::browser::{
    ///     BrowserSidecarRequest, BrowserSidecarRequestPayload,
    /// };
    ///
    /// let request = BrowserSidecarRequest::new(
    ///     "req-1",
    ///     BrowserSidecarRequestPayload::Snapshot { max_elements: None },
    /// );
    /// assert_eq!(request.id, "req-1");
    /// ```
    pub fn new(id: impl Into<String>, payload: BrowserSidecarRequestPayload) -> Self {
        Self {
            id: id.into(),
            payload,
        }
    }
}

/// One browser action request sent to the sidecar.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum BrowserSidecarRequestPayload {
    Navigate {
        url: String,
    },
    Snapshot {
        max_elements: Option<usize>,
    },
    ClickRef {
        #[serde(rename = "ref")]
        reference: String,
    },
    TypeRef {
        #[serde(rename = "ref")]
        reference: String,
        text: String,
        submit: bool,
    },
    Screenshot {
        path: String,
    },
    Close,
}

/// One browser sidecar response framed as a single JSON line.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BrowserSidecarResponse {
    pub id: String,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<BrowserSidecarResponsePayload>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<BrowserSidecarError>,
}

impl BrowserSidecarResponse {
    /// Create a successful sidecar response for one request id.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::agent::tool::browser::{
    ///     BrowserCloseResult, BrowserSidecarResponse, BrowserSidecarResponsePayload,
    /// };
    ///
    /// let response = BrowserSidecarResponse::success(
    ///     "req-1",
    ///     BrowserSidecarResponsePayload::Close(BrowserCloseResult { closed: true }),
    /// );
    /// assert!(response.ok);
    /// ```
    pub fn success(id: impl Into<String>, result: BrowserSidecarResponsePayload) -> Self {
        Self {
            id: id.into(),
            ok: true,
            result: Some(result),
            error: None,
        }
    }

    /// Create a failed sidecar response for one request id.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::agent::tool::browser::BrowserSidecarResponse;
    ///
    /// let response = BrowserSidecarResponse::failure("req-1", "bad_request", "missing url");
    /// assert!(!response.ok);
    /// ```
    pub fn failure(
        id: impl Into<String>,
        code: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            ok: false,
            result: None,
            error: Some(BrowserSidecarError::new(code, message)),
        }
    }
}

/// Successful response payload returned by one browser action.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum BrowserSidecarResponsePayload {
    Navigate(BrowserNavigateResult),
    Snapshot(BrowserSnapshotResult),
    ClickRef(BrowserActionResult),
    TypeRef(BrowserTypeResult),
    Screenshot(BrowserScreenshotResult),
    Close(BrowserCloseResult),
}

/// Structured sidecar error details.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BrowserSidecarError {
    pub code: String,
    pub message: String,
    #[serde(default)]
    pub details: Value,
}

impl BrowserSidecarError {
    /// Create a sidecar error without extra structured details.
    ///
    /// # 示例
    /// ```rust
    /// use openjarvis::agent::tool::browser::BrowserSidecarError;
    ///
    /// let error = BrowserSidecarError::new("missing_ref", "unknown ref");
    /// assert_eq!(error.code, "missing_ref");
    /// ```
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            details: Value::Null,
        }
    }
}

/// Result returned after a successful `navigate`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BrowserNavigateResult {
    pub url: String,
    pub title: String,
}

/// Result returned after a successful `snapshot`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BrowserSnapshotResult {
    pub url: String,
    pub title: String,
    pub snapshot_text: String,
    pub elements: Vec<BrowserSnapshotElement>,
    pub total_candidate_count: usize,
    pub truncated: bool,
}

/// One interactable element listed inside a browser snapshot.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BrowserSnapshotElement {
    #[serde(rename = "ref")]
    pub reference: String,
    pub tag_name: String,
    pub role: String,
    pub label: String,
    pub text: String,
    pub selector: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub href: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub section_hint: Option<String>,
    #[serde(default)]
    pub disabled: bool,
}

/// Result returned after a successful `click_ref`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BrowserActionResult {
    #[serde(rename = "ref")]
    pub reference: String,
    pub url: String,
    pub title: String,
    pub opened_new_page: bool,
}

/// Result returned after a successful `type_ref`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BrowserTypeResult {
    #[serde(rename = "ref")]
    pub reference: String,
    pub url: String,
    pub title: String,
    pub text_length: usize,
    pub submitted: bool,
    pub opened_new_page: bool,
}

/// Result returned after a successful `screenshot`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BrowserScreenshotResult {
    pub url: String,
    pub title: String,
    pub path: String,
}

/// Result returned after a successful `close`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BrowserCloseResult {
    pub closed: bool,
}
