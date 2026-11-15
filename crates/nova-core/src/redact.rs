//! Hiding secrets in captured traffic before it leaves NovaProxy.
//!
//! Two exits need it: HAR exports, which end up attached to tickets and chat
//! threads, and the MCP endpoint, which hands traffic to an AI client. Both get
//! the same treatment, applied to a copy — the engine's own flows keep their
//! values, so replays still authenticate.
//!
//! What counts as a secret is decided by *name* (a header, a query parameter, a
//! JSON key or form field) plus one shape that is a secret whatever it is called:
//! a JWT. Values are replaced, never removed, so the reader still sees that a
//! credential was sent and where; scheme words (`Bearer`) and cookie names stay
//! because they are what a person debugging auth needs to see.

use nova_proto::{BodyPreview, Flow, Header};
use serde_json::Value;

/// What a hidden value is replaced with.
pub const MARK: &str = "[redacted]";

/// Headers whose value is a credential.
const SECRET_HEADERS: [&str; 14] = [
    "authorization",
    "proxy-authorization",
    "cookie",
    "set-cookie",
    "x-api-key",
    "api-key",
    "x-auth-token",
    "x-access-token",
    "x-csrf-token",
    "x-xsrf-token",
    "x-amz-security-token",
    "x-goog-api-key",
    "x-session-token",
    "ocp-apim-subscription-key",
];

/// Query parameters, form fields and JSON keys whose value is a credential.
const SECRET_NAMES: [&str; 31] = [
    "access_token",
    "id_token",
    "refresh_token",
    "token",
    "auth",
    "auth_token",
    "authorization",
    "api_key",
    "apikey",
    "api-key",
    "key",
    "client_secret",
    "secret",
    "password",
    "passwd",
    "pwd",
    "passphrase",
    "session",
    "sessionid",
    "session_id",
    "code",
    "code_verifier",
    "assertion",
    "client_assertion",
    "private_key",
    "sig",
    "signature",
    "x-amz-signature",
    "x-amz-credential",
    "x-amz-security-token",
    "otp",
];

/// Whether a header carries a credential.
pub fn is_secret_header(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    SECRET_HEADERS.contains(&n.as_str())
        || n.contains("token")
        || n.contains("secret")
        || n.contains("password")
        || n.contains("api-key")
        || n.contains("apikey")
}

/// Whether a parameter, field or key name names a credential.
pub fn is_secret_name(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    SECRET_NAMES.contains(&n.as_str())
        || n.ends_with("_token")
        || n.ends_with("-token")
        || n.ends_with("token")
        || n.ends_with("_secret")
        || n.ends_with("password")
        || (n.ends_with("_key") && (n.contains("api") || n.contains("private") || n.contains("secret")))
}

/// A JSON Web Token: three base64url parts, the first a JSON header.
pub fn looks_like_jwt(v: &str) -> bool {
    v.starts_with("eyJ")
        && v.len() > 30
        && v.split('.').count() == 3
        && v.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'='))
}

/// A header value with its secret replaced, keeping what helps debugging:
/// the auth scheme, cookie names and `Set-Cookie` attributes.
pub fn header_value(name: &str, value: &str) -> String {
    let n = name.to_ascii_lowercase();
    match n.as_str() {
        "authorization" | "proxy-authorization" => match value.split_once(' ') {
            Some((scheme, _)) if !scheme.is_empty() => format!("{scheme} {MARK}"),
            _ => MARK.to_string(),
        },
        "cookie" => value
            .split(';')
            .map(|pair| match pair.split_once('=') {
                Some((k, _)) => format!("{}={MARK}", k.trim()),
                None => pair.trim().to_string(),
            })
            .collect::<Vec<_>>()
            .join("; "),
        "set-cookie" => {
            let mut parts = value.splitn(2, ';');
            let first = parts.next().unwrap_or("");
            let name = first.split_once('=').map(|(k, _)| k.trim()).unwrap_or(first.trim());
            match parts.next() {
                Some(attrs) => format!("{name}={MARK};{attrs}"),
                None => format!("{name}={MARK}"),
            }
        }
        _ => MARK.to_string(),
    }
}

/// Headers with every credential value replaced.
pub fn headers(hs: &[Header]) -> Vec<Header> {
    hs.iter()
        .map(|h| Header {
            name: h.name.clone(),
            value: if is_secret_header(&h.name) || looks_like_jwt(h.value.trim()) {
                header_value(&h.name, &h.value)
            } else {
                h.value.clone()
            },
        })
        .collect()
}

/// A query string (or `application/x-www-form-urlencoded` body) with secret
/// values replaced. Parameter order and names are kept.
pub fn query(qs: &str) -> String {
    qs.split('&')
        .map(|pair| match pair.split_once('=') {
            Some((k, v)) if is_secret_name(&decode_component(k)) || looks_like_jwt(&decode_component(v)) => {
                format!("{k}={MARK}")
            }
            _ => pair.to_string(),
        })
        .collect::<Vec<_>>()
        .join("&")
}

/// A URL or path with the secret values in its query replaced.
pub fn url(u: &str) -> String {
    let (before_frag, frag) = match u.split_once('#') {
        Some((a, f)) => (a, Some(f)),
        None => (u, None),
    };
    let out = match before_frag.split_once('?') {
        Some((base, qs)) => format!("{base}?{}", query(qs)),
        None => before_frag.to_string(),
    };
    match frag {
        Some(f) => format!("{out}#{f}"),
        None => out,
    }
}

/// A body's text with secrets replaced, read according to its media type.
/// JSON is walked key by key; a JSON preview cut off mid-document is scanned
/// for `"key": "value"` pairs instead. Form bodies are treated like a query.
pub fn body_text(text: &str, media_type: Option<&str>) -> String {
    let media = media_type.unwrap_or("").to_ascii_lowercase();
    if media.contains("x-www-form-urlencoded") {
        return query(text);
    }
    let trimmed = text.trim_start();
    if media.contains("json") || trimmed.starts_with('{') || trimmed.starts_with('[') {
        if let Ok(mut v) = serde_json::from_str::<Value>(text) {
            if json_value(&mut v) {
                let pretty = text.contains('\n');
                return if pretty { serde_json::to_string_pretty(&v) } else { serde_json::to_string(&v) }
                    .unwrap_or_else(|_| text.to_string());
            }
            return text.to_string();
        }
        return json_text_scan(text);
    }
    text.to_string()
}

/// Replace secret values inside a parsed JSON value. Returns whether anything
/// changed, so an untouched body keeps its exact formatting.
fn json_value(v: &mut Value) -> bool {
    match v {
        Value::Object(map) => {
            let mut changed = false;
            for (k, val) in map.iter_mut() {
                if is_secret_name(k)
                    && !matches!(val, Value::Null | Value::Bool(_) | Value::Object(_) | Value::Array(_))
                {
                    *val = Value::String(MARK.into());
                    changed = true;
                } else {
                    changed |= json_value(val);
                }
            }
            changed
        }
        Value::Array(items) => items.iter_mut().fold(false, |c, i| json_value(i) | c),
        Value::String(s) if looks_like_jwt(s) => {
            *s = MARK.into();
            true
        }
        _ => false,
    }
}

/// For JSON that does not parse (a truncated preview): find each
/// `"name": "value"` whose name is a secret and blank the string value.
fn json_text_scan(text: &str) -> String {
    let b = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    let mut copied = 0;
    while i < b.len() {
        if b[i] != b'"' {
            i += 1;
            continue;
        }
        // A string literal: find its end.
        let start = i + 1;
        let Some(end) = string_end(b, start) else { break };
        let name = &text[start..end];
        // Is it a key followed by a string value?
        let mut j = end + 1;
        while j < b.len() && b[j].is_ascii_whitespace() {
            j += 1;
        }
        if j < b.len() && b[j] == b':' {
            j += 1;
            while j < b.len() && b[j].is_ascii_whitespace() {
                j += 1;
            }
            if j < b.len() && b[j] == b'"' && is_secret_name(name) {
                let vstart = j + 1;
                let vend = string_end(b, vstart).unwrap_or(b.len());
                out.push_str(&text[copied..vstart]);
                out.push_str(MARK);
                copied = vend;
                i = vend + 1;
                continue;
            }
        }
        if looks_like_jwt(name) {
            out.push_str(&text[copied..start]);
            out.push_str(MARK);
            copied = end;
        }
        i = end + 1;
    }
    out.push_str(&text[copied.min(text.len())..]);
    out
}

/// Index of the closing quote of a JSON string whose content starts at `from`.
fn string_end(b: &[u8], from: usize) -> Option<usize> {
    let mut k = from;
    while k < b.len() {
        match b[k] {
            b'\\' => k += 2,
            b'"' => return Some(k),
            _ => k += 1,
        }
    }
    None
}

/// A URL component decoded like `URLSearchParams` does: `%XX` escapes, and `+`
/// for a space.
pub fn decode_component(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' if i + 2 < b.len() => match u8::from_str_radix(&s[i + 1..i + 3], 16) {
                Ok(v) => {
                    out.push(v);
                    i += 3;
                }
                Err(_) => {
                    out.push(b'%');
                    i += 1;
                }
            },
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A body preview with secrets replaced in its text. Binary bodies are kept:
/// there is nothing to read a name from.
pub fn body(b: &BodyPreview) -> BodyPreview {
    let mut out = b.clone();
    if let Some(t) = &b.text {
        out.text = Some(body_text(t, b.media_type.as_deref()));
    }
    out
}

/// A copy of `flow` that is safe to hand out: headers, URL, path and body
/// texts with every recognised secret replaced.
pub fn flow(f: &Flow) -> Flow {
    let mut out = f.clone();
    out.url = url(&f.url);
    out.path = url(&f.path);
    out.request_headers = headers(&f.request_headers);
    out.response_headers = headers(&f.response_headers);
    out.request_body = f.request_body.as_ref().map(body);
    out.response_body = f.response_body.as_ref().map(body);
    if let Some(m) = &f.mapped_from {
        out.mapped_from = Some(url(m));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_headers_keep_their_scheme_and_cookies_their_names() {
        assert_eq!(header_value("Authorization", "Bearer abc.def"), "Bearer [redacted]");
        assert_eq!(header_value("Authorization", "opaque"), "[redacted]");
        assert_eq!(header_value("Cookie", "sid=1; theme=dark"), "sid=[redacted]; theme=[redacted]");
        assert_eq!(header_value("Set-Cookie", "sid=1; Path=/; HttpOnly"), "sid=[redacted]; Path=/; HttpOnly");
        let hs = headers(&[
            Header { name: "X-Api-Key".into(), value: "k".into() },
            Header { name: "Accept".into(), value: "*/*".into() },
            Header { name: "X-Debug".into(), value: "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxIn0.c2lnbmF0dXJl".into() },
        ]);
        assert_eq!(hs[0].value, MARK);
        assert_eq!(hs[1].value, "*/*");
        assert_eq!(hs[2].value, MARK, "a JWT is a secret whatever the header is called");
    }

    #[test]
    fn query_secrets_are_replaced_and_the_rest_kept() {
        assert_eq!(
            url("https://a.test/cb?code=xyz&state=1&access_token=t#frag"),
            "https://a.test/cb?code=[redacted]&state=1&access_token=[redacted]#frag"
        );
        assert_eq!(url("/p?q=hello&x"), "/p?q=hello&x");
        assert_eq!(query("API%5FKEY=1&n=2"), "API%5FKEY=[redacted]&n=2");
    }

    #[test]
    fn json_bodies_are_walked_and_keep_their_formatting_when_untouched() {
        let out =
            body_text(r#"{"user":"a","password":"p","nested":{"refresh_token":"r","n":1}}"#, Some("application/json"));
        let got: Value = serde_json::from_str(&out).unwrap();
        let want = serde_json::json!({"user": "a", "password": MARK, "nested": {"refresh_token": MARK, "n": 1}});
        assert_eq!(got, want);
        let untouched = "{ \"a\": 1 }";
        assert_eq!(body_text(untouched, Some("application/json")), untouched);
    }

    #[test]
    fn a_truncated_json_preview_is_scanned_instead() {
        let cut = r#"{"client_secret": "s3cr\"et", "name": "n", "items": [1, 2"#;
        assert_eq!(
            body_text(cut, Some("application/json")),
            r#"{"client_secret": "[redacted]", "name": "n", "items": [1, 2"#
        );
    }

    #[test]
    fn form_bodies_are_treated_like_a_query() {
        assert_eq!(
            body_text("grant_type=password&username=u&password=p", Some("application/x-www-form-urlencoded")),
            "grant_type=password&username=u&password=[redacted]"
        );
    }

    #[test]
    fn plain_names_are_not_secrets() {
        for n in ["q", "page", "user", "state", "monkey", "keyword", "tokenizer"] {
            assert!(!is_secret_name(n), "{n}");
        }
        for n in ["csrf_token", "github_token", "stripe_api_key", "AccessToken", "newPassword"] {
            assert!(is_secret_name(n), "{n}");
        }
    }
}
