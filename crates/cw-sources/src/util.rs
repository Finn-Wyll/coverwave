//! Small helpers: HTTP agent, URL encoding, base64url, browser opening.
#![cfg_attr(not(feature = "http"), allow(dead_code))]

#[cfg(feature = "http")]
use std::time::Duration;

#[cfg(feature = "http")]
pub fn agent() -> ureq::Agent {
    use ureq::tls::{RootCerts, TlsConfig};
    ureq::Agent::config_builder()
        .tls_config(
            TlsConfig::builder()
                .root_certs(RootCerts::PlatformVerifier)
                .build(),
        )
        .timeout_global(Some(Duration::from_secs(10)))
        // Status codes are handled by the callers.
        .http_status_as_error(false)
        .build()
        .into()
}

#[cfg(feature = "http")]
pub fn http_get_bytes(url: &str) -> Result<Vec<u8>, String> {
    let resp = agent().get(url).call().map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }
    resp.into_body()
        .with_config()
        .limit(20 << 20)
        .read_to_vec()
        .map_err(|e| e.to_string())
}

/// Percent-encodes everything except RFC 3986 unreserved characters.
pub fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Decodes `%XX` and `+` (as space).
pub fn decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => {
                match u8::from_str_radix(
                    std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or(""),
                    16,
                ) {
                    Ok(v) => {
                        out.push(v);
                        i += 2;
                    }
                    Err(_) => out.push(b'%'),
                }
            }
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `key=value&...` from pairs, values encoded.
pub fn form(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| format!("{k}={}", encode(v)))
        .collect::<Vec<_>>()
        .join("&")
}

/// The value of `key` in a query string.
pub fn query_param(query: &str, key: &str) -> Option<String> {
    query.split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=')?;
        (k == key).then(|| decode(v))
    })
}

/// Base64url without padding (RFC 4648 section 5).
pub fn base64url(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(data.len() * 4 / 3 + 3);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        let chars = chunk.len() + 1;
        for k in 0..chars {
            out.push(T[(n >> (18 - 6 * k) & 63) as usize] as char);
        }
    }
    out
}

/// Opens `url` in the default browser. Returns false if that failed (the
/// caller then shows the URL).
pub fn open_browser(url: &str) -> bool {
    // The usual convention: $BROWSER, when set, opens URLs.
    if let Some(b) = std::env::var_os("BROWSER").filter(|b| !b.is_empty()) {
        return std::process::Command::new(b)
            .arg(url)
            .status()
            .is_ok_and(|s| s.success());
    }
    #[cfg(target_os = "macos")]
    let r = std::process::Command::new("open").arg(url).status();
    // Not `cmd /C start`: cmd would split the URL at its `&`s.
    #[cfg(windows)]
    let r = std::process::Command::new("rundll32")
        .args(["url.dll,FileProtocolHandler", url])
        .status();
    #[cfg(all(not(target_os = "macos"), not(windows)))]
    let r = std::process::Command::new("xdg-open").arg(url).status();
    r.is_ok_and(|s| s.success())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_encoding_round_trip() {
        assert_eq!(encode("a b/c?d=é"), "a%20b%2Fc%3Fd%3D%C3%A9");
        assert_eq!(decode("a%20b+c%2F%C3%A9"), "a b c/é");
        assert_eq!(decode("100%"), "100%");
        assert_eq!(
            query_param("code=AB%2Fc&state=xyz", "code").as_deref(),
            Some("AB/c")
        );
        assert_eq!(query_param("code=1", "state"), None);
    }

    #[test]
    fn base64url_vectors() {
        assert_eq!(base64url(b""), "");
        assert_eq!(base64url(b"f"), "Zg");
        assert_eq!(base64url(b"fo"), "Zm8");
        assert_eq!(base64url(b"foo"), "Zm9v");
        assert_eq!(base64url(&[0xfb, 0xff]), "-_8");
    }
}
