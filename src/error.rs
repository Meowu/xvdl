use serde::Serialize;
use thiserror::Error;

/// A stable error category shared by the library, CLI, and Worker adapters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    InvalidInput,
    NoVideo,
    Upstream,
    InvalidResponse,
    Internal,
}

/// Errors contain a short message for programs and an optional hint for people.
///
/// We deliberately do not expose `reqwest::Error` in the public API. That keeps
/// callers independent from the HTTP implementation and prevents accidental
/// serialization of low-level details from the Worker.
#[derive(Debug, Clone, Error, PartialEq, Eq)]
#[error("{message}")]
pub struct XReadError {
    pub kind: ErrorKind,
    pub message: String,
    pub hint: Option<String>,
    /// HTTP status returned by the upstream service, when there was one.
    pub upstream_status: Option<u16>,
}

impl XReadError {
    pub fn invalid_input(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::InvalidInput, message)
    }

    pub fn upstream(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Upstream, message)
    }

    pub fn no_video(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::NoVideo, message)
    }

    pub fn invalid_response(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::InvalidResponse, message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Internal, message)
    }

    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    pub fn with_upstream_status(mut self, status: u16) -> Self {
        self.upstream_status = Some(status);
        self
    }

    /// Translate domain failures into a safe status for the public Worker API.
    pub fn response_status(&self) -> u16 {
        match self.kind {
            ErrorKind::InvalidInput => 400,
            ErrorKind::NoVideo => 404,
            ErrorKind::Upstream | ErrorKind::InvalidResponse => 502,
            ErrorKind::Internal => 500,
        }
    }

    fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            hint: None,
            upstream_status: None,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ErrorResponse<'a> {
    pub error: &'a str,
    pub kind: ErrorKind,
    pub hint: Option<&'a str>,
}

impl<'a> From<&'a XReadError> for ErrorResponse<'a> {
    fn from(error: &'a XReadError) -> Self {
        Self {
            error: &error.message,
            kind: error.kind,
            hint: error.hint.as_deref(),
        }
    }
}
