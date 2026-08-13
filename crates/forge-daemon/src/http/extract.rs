//! Extractors that fail in the API's own error shape.
//!
//! Axum's built-in rejections answer in plain text, which would make the
//! documented error envelope a half-truth and hand the app a string it cannot
//! render as a message.

use axum::extract::rejection::QueryRejection;
use axum::extract::{FromRequestParts, Query as AxumQuery};
use axum::http::request::Parts;
use serde::de::DeserializeOwned;

use super::error::ApiError;

/// `axum::extract::Query`, but a malformed query string is a `bad_request`
/// envelope rather than plain text.
#[derive(Debug, Clone, Copy)]
pub struct Query<T>(pub T);

impl<S, T> FromRequestParts<S> for Query<T>
where
    S: Send + Sync,
    T: DeserializeOwned,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        AxumQuery::<T>::from_request_parts(parts, state)
            .await
            .map(|AxumQuery(value)| Self(value))
            .map_err(describe)
    }
}

fn describe(rejection: QueryRejection) -> ApiError {
    ApiError::bad_request(format!(
        "the query string is not valid for this endpoint: {}",
        rejection.body_text()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::Request;
    use axum::http::StatusCode;

    #[derive(serde::Deserialize)]
    struct Cursor {
        after: Option<i64>,
    }

    async fn extract(uri: &str) -> Result<Option<i64>, ApiError> {
        let (mut parts, _) = Request::builder().uri(uri).body(()).unwrap().into_parts();

        Query::<Cursor>::from_request_parts(&mut parts, &())
            .await
            .map(|Query(cursor)| cursor.after)
    }

    #[tokio::test]
    async fn a_valid_query_string_parses() {
        assert_eq!(extract("/events?after=5").await.unwrap(), Some(5));
        assert_eq!(extract("/events").await.unwrap(), None);
    }

    #[tokio::test]
    async fn a_malformed_value_is_a_bad_request_not_plain_text() {
        let error = extract("/events?after=soon").await.unwrap_err();

        assert_eq!(error.status(), StatusCode::BAD_REQUEST);
    }
}
