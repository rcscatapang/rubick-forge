//! Bearer-token authentication.
//!
//! The header is the rule. Browsers cannot set headers on a WebSocket
//! handshake, so `?token=` is accepted on `/ws/` routes only — a secret in a
//! URL is a secret in access logs, and REST clients have no excuse for it.

use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::Response;

use super::error::{ApiError, ApiResult};
use super::AppState;

const QUERY_KEY: &str = "token";
const WS_PREFIX: &str = "/ws/";

pub async fn require_token(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> ApiResult<Response> {
    let presented = from_header(&request).or_else(|| from_query(&request));

    match presented {
        Some(presented) if state.token.matches(&presented) => Ok(next.run(request).await),
        Some(_) => Err(ApiError::unauthorized(
            "that bearer token is not this daemon's",
        )),
        None => Err(ApiError::unauthorized(
            "this endpoint needs an `Authorization: Bearer <token>` header",
        )),
    }
}

fn from_header(request: &Request) -> Option<String> {
    let value = request
        .headers()
        .get(axum::http::header::AUTHORIZATION)?
        .to_str()
        .ok()?;

    // The scheme is case-insensitive per RFC 7235.
    let (scheme, credential) = value.split_once(' ')?;
    scheme
        .eq_ignore_ascii_case("bearer")
        .then(|| credential.trim().to_owned())
}

fn from_query(request: &Request) -> Option<String> {
    if !request.uri().path().starts_with(WS_PREFIX) {
        return None;
    }

    let query = request.uri().query()?;
    query.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        (key == QUERY_KEY).then(|| percent_decode(value))
    })
}

/// Tokens are hex, but a client may still percent-encode them; decoding keeps
/// a correct client from being rejected over an encoding detail.
fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;

    while index < bytes.len() {
        match bytes[index] {
            b'%' if index + 2 < bytes.len() => match hex_pair(bytes[index + 1], bytes[index + 2]) {
                Some(byte) => {
                    out.push(byte);
                    index += 3;
                }
                None => {
                    out.push(b'%');
                    index += 1;
                }
            },
            b'+' => {
                out.push(b' ');
                index += 1;
            }
            byte => {
                out.push(byte);
                index += 1;
            }
        }
    }

    String::from_utf8_lossy(&out).into_owned()
}

/// Decoding byte-wise rather than by slicing the `&str`, which would panic on
/// a multi-byte character boundary.
fn hex_pair(high: u8, low: u8) -> Option<u8> {
    Some((hex_digit(high)? << 4) | hex_digit(low)?)
}

fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;

    fn request(builder: axum::http::request::Builder) -> Request {
        builder.body(Body::empty()).unwrap()
    }

    #[test]
    fn a_bearer_header_is_read_whatever_its_casing() {
        for scheme in ["Bearer", "bearer", "BEARER"] {
            let req = request(
                Request::builder()
                    .uri("/events")
                    .header("authorization", format!("{scheme} abc123")),
            );
            assert_eq!(from_header(&req).as_deref(), Some("abc123"));
        }
    }

    #[test]
    fn other_schemes_are_ignored() {
        let req = request(
            Request::builder()
                .uri("/events")
                .header("authorization", "Basic abc123"),
        );
        assert_eq!(from_header(&req), None);
    }

    #[test]
    fn a_websocket_route_accepts_a_query_token() {
        let req = request(Request::builder().uri("/ws/events?after=5&token=abc123"));
        assert_eq!(from_query(&req).as_deref(), Some("abc123"));
    }

    #[test]
    fn a_rest_route_does_not() {
        let req = request(Request::builder().uri("/events?token=abc123"));
        assert_eq!(from_query(&req), None);
    }

    #[test]
    fn a_query_without_a_token_yields_nothing() {
        let req = request(Request::builder().uri("/ws/events?after=5"));
        assert_eq!(from_query(&req), None);

        let req = request(Request::builder().uri("/ws/events"));
        assert_eq!(from_query(&req), None);
    }

    #[test]
    fn a_key_that_merely_ends_in_token_is_not_the_token() {
        let req = request(Request::builder().uri("/ws/events?mytoken=abc123"));
        assert_eq!(from_query(&req), None);
    }

    #[test]
    fn percent_encoded_values_are_decoded() {
        assert_eq!(percent_decode("ab%20cd"), "ab cd");
        assert_eq!(percent_decode("ab+cd"), "ab cd");
        assert_eq!(percent_decode("abc123"), "abc123");
        // A stray `%` is data, not a decoding failure.
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("100%zz"), "100%zz");
    }

    #[test]
    fn a_multibyte_character_does_not_panic_the_decoder() {
        // A `%` followed by bytes that are not a hex pair, where the second of
        // them starts a multi-byte character: slicing the `&str` here would
        // panic on the character boundary.
        assert_eq!(percent_decode("%zé"), "%zé");
        assert_eq!(percent_decode("é%2"), "é%2");
        assert_eq!(percent_decode("café"), "café");
    }
}
