//! Small HTTP and encoding helpers shared by the sign-in, account and export code.

use crate::{App, Error};
use axum::Json;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};

// A duplicate cookie is ambiguous (for example, a sibling subdomain's cookie),
// so never choose one based on header or path ordering.
/// The single 64-hex-digit cookie called `name`, or `None` if it is missing,
/// malformed, or sent more than once.
pub(crate) fn hex_cookie<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    let mut found = None;
    for header in headers.get_all(header::COOKIE) {
        for cookie in header.to_str().ok()?.split(';') {
            if let Some((key, value)) = cookie.trim().split_once('=')
                && key == name
            {
                if found.is_some()
                    || value.len() != 64
                    || !value.bytes().all(|b| b.is_ascii_hexdigit())
                {
                    return None;
                }
                found = Some(value);
            }
        }
    }
    found
}

/// A `Set-Cookie` value for an HttpOnly cookie, Secure when the site is served over https.
pub(crate) fn set_cookie(
    app: &App,
    name: &str,
    path: &str,
    value: &str,
    max_age: i64,
) -> HeaderValue {
    let secure = app
        .config
        .public_url
        .as_deref()
        .is_some_and(|url| url.starts_with("https://"));
    format!(
        "{name}={value}; Path={path}; HttpOnly; SameSite=Lax; Max-Age={max_age}{}",
        if secure { "; Secure" } else { "" }
    )
    .parse()
    .unwrap()
}

/// 32 random bytes; `unavailable` is the error when the system has none to give.
pub(crate) fn random_bytes(unavailable: &'static str) -> Result<[u8; 32], Error> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|_| unavailable)?;
    Ok(bytes)
}

/// Lowercase hexadecimal.
pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn base64_with(bytes: &[u8], alphabet: &[u8; 64], padded: bool) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, b)| n | u32::from(*b) << (16 - 8 * i));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(alphabet[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else if padded {
                out.push('=');
            }
        }
    }
    out
}

/// Standard, padded base64.
pub(crate) fn base64(bytes: &[u8]) -> String {
    base64_with(
        bytes,
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/",
        true,
    )
}

/// Unpadded base64url, as PKCE uses.
pub(crate) fn base64url(bytes: &[u8]) -> String {
    base64_with(
        bytes,
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_",
        false,
    )
}

/// The token of an `Authorization: Bearer` header.
pub(crate) fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
}

/// `{"error": message}` with `status`.
pub(crate) fn json_error(status: StatusCode, message: &str) -> Response {
    (status, Json(serde_json::json!({"error": message}))).into_response()
}

/// A typed user name as it is stored: trimmed and lowercased.
pub(crate) fn normalize_user(raw: &str) -> String {
    raw.trim().to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_the_standard_encoding() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }
}
