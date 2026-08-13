//! One error shape for the whole API.
//!
//! Clients render `message` directly, so it must read as a sentence a person
//! can act on — never a raw stderr dump or a debug-formatted Rust error.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;

use crate::store::StoreError;

#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: String,
}

#[derive(Serialize)]
struct Body<'a> {
    error: Detail<'a>,
}

#[derive(Serialize)]
struct Detail<'a> {
    /// Stable machine-readable discriminator.
    code: &'a str,
    message: &'a str,
}

impl ApiError {
    pub fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }

    pub fn unauthorized(message: impl Into<String>) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "unauthorized", message)
    }

    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "bad_request", message)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, "not_found", message)
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, "conflict", message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", message)
    }

    pub fn status(&self) -> StatusCode {
        self.status
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(Body {
                error: Detail {
                    code: self.code,
                    message: &self.message,
                },
            }),
        )
            .into_response()
    }
}

impl From<StoreError> for ApiError {
    /// Storage failures are the daemon's fault, and the detail belongs in the
    /// log rather than in a client's toast.
    fn from(err: StoreError) -> Self {
        tracing::error!(error = %err, "storage failure");
        Self::internal("the daemon could not read its own database; check the daemon logs")
    }
}

pub type ApiResult<T> = Result<T, ApiError>;

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;

    async fn body_of(error: ApiError) -> serde_json::Value {
        let response = error.into_response();
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn errors_render_as_a_coded_envelope() {
        let json = body_of(ApiError::not_found("no project with id 7")).await;

        assert_eq!(json["error"]["code"], "not_found");
        assert_eq!(json["error"]["message"], "no project with id 7");
    }

    #[tokio::test]
    async fn storage_failures_do_not_leak_sql_to_the_client() {
        let store_error = StoreError::Corrupt("events.payload is not JSON".into());
        let api_error = ApiError::from(store_error);

        assert_eq!(api_error.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let json = body_of(api_error).await;
        assert_eq!(json["error"]["code"], "internal");
        assert!(!json["error"]["message"]
            .as_str()
            .unwrap()
            .contains("payload"));
    }

    #[test]
    fn each_constructor_carries_its_status() {
        assert_eq!(
            ApiError::unauthorized("x").status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(ApiError::bad_request("x").status(), StatusCode::BAD_REQUEST);
        assert_eq!(ApiError::not_found("x").status(), StatusCode::NOT_FOUND);
    }
}
