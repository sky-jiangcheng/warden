//! Structured errors at the IPC boundary.
//!
//! Phase 6 open question 8, decided as option 2: internal code uses a
//! `thiserror` enum (`AppError`), and the IPC boundary serialises a stable
//! `{ code, message, data? }` (`ApiError`) so the frontend can branch on the
//! category instead of pattern-matching English phrases. Anything that
//! never got a category lands on `INTERNAL_ERROR` — the unknown-error
//! fallback — rather than being silently reclassified.
//!
//! The categories are the ones the UI actually acts on, and each names a
//! different action: not-installed gets installation guidance, authentication
//! failures jump to settings, a dead connection says so. A `String` that
//! happens to contain "connection refused" is not the same contract as a
//! `Connection` code, because the phrase can appear in an unrelated error.

use serde::Serialize;
use thiserror::Error;

/// The stable machine-readable categories crossing the IPC boundary.
///
/// The serialised spelling is part of the contract: the frontend branches on
/// it, so renaming a variant is a breaking change, not a refactor. Keep the
/// `SCREAMING_SNAKE_CASE` names stable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    /// A runtime this product needs is not on this machine.
    NotInstalled,
    /// The endpoint could not be reached or did not answer usefully.
    Connection,
    /// Credentials are missing, wrong, or rejected.
    Authentication,
    /// The request exceeded its time budget.
    Timeout,
    /// The peer's contract is not one this client can speak.
    ContractMismatch,
    /// The caller's input is not valid.
    InvalidInput,
    /// The requested thing does not exist.
    NotFound,
    /// The action was refused — by the user, or by a gate.
    Denied,
    /// A known failure with no finer category.
    Failed,
    /// Anything that never got a category.
    InternalError,
}

impl ErrorCode {
    /// The wire spelling. Exposed for tests and for the tracing event.
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorCode::NotInstalled => "NOT_INSTALLED",
            ErrorCode::Connection => "CONNECTION",
            ErrorCode::Authentication => "AUTHENTICATION",
            ErrorCode::Timeout => "TIMEOUT",
            ErrorCode::ContractMismatch => "CONTRACT_MISMATCH",
            ErrorCode::InvalidInput => "INVALID_INPUT",
            ErrorCode::NotFound => "NOT_FOUND",
            ErrorCode::Denied => "DENIED",
            ErrorCode::Failed => "FAILED",
            ErrorCode::InternalError => "INTERNAL_ERROR",
        }
    }
}

/// The internal error type. Logic in the modules raises these; the IPC
/// boundary converts them to [`ApiError`].
#[derive(Debug, Error)]
pub enum AppError {
    #[error("{0}")]
    NotInstalled(String),
    #[error("{0}")]
    Connection(String),
    #[error("{0}")]
    Authentication(String),
    #[error("{0}")]
    Timeout(String),
    #[error("{0}")]
    ContractMismatch(String),
    #[error("{0}")]
    InvalidInput(String),
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    Denied(String),
    #[error("{0}")]
    Failed(String),
    #[error("{0}")]
    Internal(String),
}

impl AppError {
    /// The code this variant carries across the boundary.
    pub fn code(&self) -> ErrorCode {
        match self {
            AppError::NotInstalled(_) => ErrorCode::NotInstalled,
            AppError::Connection(_) => ErrorCode::Connection,
            AppError::Authentication(_) => ErrorCode::Authentication,
            AppError::Timeout(_) => ErrorCode::Timeout,
            AppError::ContractMismatch(_) => ErrorCode::ContractMismatch,
            AppError::InvalidInput(_) => ErrorCode::InvalidInput,
            AppError::NotFound(_) => ErrorCode::NotFound,
            AppError::Denied(_) => ErrorCode::Denied,
            AppError::Failed(_) => ErrorCode::Failed,
            AppError::Internal(_) => ErrorCode::InternalError,
        }
    }
}

/// The error shape the frontend receives: a stable code it can branch on,
/// a human message it shows verbatim, and optional structured data.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiError {
    pub code: ErrorCode,
    pub message: String,
    /// Optional structured detail (for example the failing tool name).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

impl ApiError {
    /// Builds an error from a category and a message.
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }

    /// Attaches structured detail.
    pub fn with_data(mut self, data: serde_json::Value) -> Self {
        self.data = Some(data);
        self
    }

    /// Emits the boundary tracing event.
    ///
    /// One event per failed command, carrying the fields the acceptance
    /// criterion asks to filter by: `code`, `provider` and `thread` where
    /// the command knows them. This is deliberately a thin addition at the
    /// boundary rather than a logging migration — the decision is to satisfy
    /// filterability, not to rebuild observability.
    pub fn report(&self, command: &str, provider: Option<&str>, thread: Option<&str>) {
        tracing::warn!(
            code = self.code.as_str(),
            message = %self.message,
            command,
            provider,
            thread,
            "command failed"
        );
    }
}

/// Wraps a command's result so a failure is logged at the IPC boundary with
/// its stable code and, where the command knows them, the provider and thread
/// it was acting on.
///
/// This is the single place a failed command becomes a log line, which keeps
/// the event shape uniform and lets the log be filtered by `code`, `provider`
/// or `thread` without touching each command's error path.
pub fn boundary<T>(
    command: &str,
    result: Result<T, ApiError>,
    provider: Option<&str>,
    thread: Option<&str>,
) -> Result<T, ApiError> {
    if let Err(err) = &result {
        err.report(command, provider, thread);
    }
    result
}

impl From<AppError> for ApiError {
    fn from(error: AppError) -> Self {
        Self::new(error.code(), error.to_string())
    }
}

/// The fallback for errors that never got a category: `INTERNAL_ERROR`.
///
/// This is what makes an unclassified `String` honest rather than
/// reclassified — the frontend sees a code it does not pretend to
/// understand, and the message still reaches the user verbatim.
impl From<String> for ApiError {
    fn from(message: String) -> Self {
        Self::new(ErrorCode::InternalError, message)
    }
}

/// Tauri's own errors (window dragging, surface checks) are infrastructure
/// failures with no user-facing category: they are reported as-is under
/// `INTERNAL_ERROR` so the message still reaches the user.
impl From<tauri::Error> for ApiError {
    fn from(error: tauri::Error) -> Self {
        Self::new(ErrorCode::InternalError, error.to_string())
    }
}

impl From<&str> for ApiError {
    fn from(message: &str) -> Self {
        Self::new(ErrorCode::InternalError, message)
    }
}

/// Maps a model-client failure onto a boundary code.
///
/// The one place where an English phrase would be wrong: `Transport` and
/// `Status` need different actions (fix the network vs. fix the key), and a
/// `401/403` is an authentication problem regardless of what the provider
/// put in its message. Mapping by variant, not by substring.
impl From<crate::llm::LlmError> for ApiError {
    fn from(error: crate::llm::LlmError) -> Self {
        match error {
            crate::llm::LlmError::Transport(message) => Self::new(ErrorCode::Connection, message),
            crate::llm::LlmError::Timeout(message) => Self::new(ErrorCode::Timeout, message),
            crate::llm::LlmError::Status { code, message } => {
                // 401/403 are the credential answers; everything else is a
                // provider-side failure the user cannot act on beyond
                // "check the provider".
                if code == 401 || code == 403 {
                    Self::new(ErrorCode::Authentication, format!("{code}: {message}"))
                } else {
                    Self::new(ErrorCode::Connection, format!("{code}: {message}"))
                }
            }
            crate::llm::LlmError::Protocol(message) => {
                Self::new(ErrorCode::ContractMismatch, message)
            }
        }
    }
}

/// Maps a memory-layer failure onto a boundary code.
///
/// `NotInstalled` is the actionable one — it is what the memory card turns
/// into installation guidance — so it must not collapse into a generic
/// connection failure.
impl From<crate::mcp_client::McpError> for ApiError {
    fn from(error: crate::mcp_client::McpError) -> Self {
        match error {
            crate::mcp_client::McpError::NotInstalled => Self::new(
                ErrorCode::NotInstalled,
                "the memory layer server is not installed",
            ),
            crate::mcp_client::McpError::Timeout(message) => Self::new(ErrorCode::Timeout, message),
            crate::mcp_client::McpError::Spawn(io) => Self::new(
                ErrorCode::Connection,
                format!("the server could not be started: {io}"),
            ),
            crate::mcp_client::McpError::Protocol(message)
            | crate::mcp_client::McpError::Malformed(message) => {
                Self::new(ErrorCode::ContractMismatch, message)
            }
            crate::mcp_client::McpError::Closed(message) => {
                Self::new(ErrorCode::Connection, message)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wire_spellings_are_stable() {
        // The frontend branches on these strings; a rename is a breaking
        // change. Pin them so a future edit cannot silently change the
        // contract.
        assert_eq!(ErrorCode::NotInstalled.as_str(), "NOT_INSTALLED");
        assert_eq!(ErrorCode::Connection.as_str(), "CONNECTION");
        assert_eq!(ErrorCode::Authentication.as_str(), "AUTHENTICATION");
        assert_eq!(ErrorCode::Timeout.as_str(), "TIMEOUT");
        assert_eq!(ErrorCode::ContractMismatch.as_str(), "CONTRACT_MISMATCH");
        assert_eq!(ErrorCode::InvalidInput.as_str(), "INVALID_INPUT");
        assert_eq!(ErrorCode::NotFound.as_str(), "NOT_FOUND");
        assert_eq!(ErrorCode::Denied.as_str(), "DENIED");
        assert_eq!(ErrorCode::Failed.as_str(), "FAILED");
        assert_eq!(ErrorCode::InternalError.as_str(), "INTERNAL_ERROR");
    }

    #[test]
    fn the_code_follows_the_variant() {
        assert_eq!(
            AppError::NotInstalled("x".into()).code(),
            ErrorCode::NotInstalled
        );
        assert_eq!(
            AppError::Authentication("x".into()).code(),
            ErrorCode::Authentication
        );
        assert_eq!(
            AppError::InvalidInput("x".into()).code(),
            ErrorCode::InvalidInput
        );
        assert_eq!(
            AppError::Internal("x".into()).code(),
            ErrorCode::InternalError
        );
    }

    #[test]
    fn an_uncategorised_string_falls_back_to_internal() {
        // The unknown-error fallback: the message survives, the code does
        // not pretend to a category the producer never chose.
        let error: ApiError = "something odd".to_string().into();
        assert_eq!(error.code, ErrorCode::InternalError);
        assert_eq!(error.message, "something odd");
    }

    #[test]
    fn the_payload_is_the_contract_shape() {
        // { code, message } with data omitted when absent, and included when
        // present. The frontend reads exactly these fields.
        let bare = serde_json::to_value(ApiError::new(ErrorCode::NotFound, "gone")).unwrap();
        assert_eq!(bare["code"], "NOT_FOUND");
        assert_eq!(bare["message"], "gone");
        assert!(bare.get("data").is_none(), "absent data must not serialize");

        let with = serde_json::to_value(
            ApiError::new(ErrorCode::Denied, "no").with_data(serde_json::json!({ "tool": "x" })),
        )
        .unwrap();
        assert_eq!(with["code"], "DENIED");
        assert_eq!(with["data"]["tool"], "x");
    }
}
