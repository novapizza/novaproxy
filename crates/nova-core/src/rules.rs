//! Traffic-control rules: matching + application. Pure logic over `http` parts
//! and [`nova_proto::Rule`], so it stays unit-testable without a live proxy.

use hudsucker::hyper::header::{HeaderName, HeaderValue};
use hudsucker::hyper::http::request::Parts;
use hudsucker::hyper::Uri;
use nova_proto::{Rule, RuleKind};

/// The decision produced by evaluating the rule set against a request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Forward the (possibly-mutated) request. `mapped_from` is set if a
    /// Map Remote rule rewrote the destination.
    Forward { mapped_from: Option<String> },
    /// Short-circuit with a 403.
    Block,
    /// Short-circuit by serving a local file.
    MapLocal(String),
}

/// Glob match where `*` matches any run of characters. Case-insensitive on host.
pub fn glob_match(pattern: &str, text: &str) -> bool {
    // Split on '*' and require the literal segments to appear in order, with the
    // first/last anchored unless the pattern starts/ends with '*'.
    let p = pattern.to_ascii_lowercase();
    let t = text.to_ascii_lowercase();
    let parts: Vec<&str> = p.split('*').collect();
    if parts.len() == 1 {
        return p == t;
    }
    let mut pos = 0usize;
    for (i, seg) in parts.iter().enumerate() {
        if seg.is_empty() {
            continue;
        }
        if i == 0 {
            if !t[pos..].starts_with(seg) {
                return false;
            }
            pos += seg.len();
        } else if i == parts.len() - 1 {
            if !t[pos..].ends_with(seg) {
                return false;
            }
        } else if let Some(found) = t[pos..].find(seg) {
            pos += found + seg.len();
        } else {
            return false;
        }
    }
    true
}

/// Evaluate `rules` against a request, mutating `parts` for Rewrite / Map Remote.
/// `url` is the reconstructed `scheme://host/path` used for matching.
pub fn apply_request(rules: &[Rule], url: &str, parts: &mut Parts) -> Outcome {
    let mut mapped_from: Option<String> = None;

    for rule in rules.iter().filter(|r| r.enabled) {
        if !glob_match(&rule.pattern, url) {
            continue;
        }
        match rule.kind {
            RuleKind::Block => return Outcome::Block,
            RuleKind::MapLocal => {
                if let Some(path) = rule.target.clone().filter(|s| !s.is_empty()) {
                    return Outcome::MapLocal(path);
                }
            }
            RuleKind::Rewrite => {
                if let (Some(name), Some(value)) = (&rule.header_name, &rule.header_value) {
                    if let (Ok(n), Ok(v)) =
                        (HeaderName::try_from(name.as_str()), HeaderValue::try_from(value.as_str()))
                    {
                        parts.headers.insert(n, v);
                    }
                }
            }
            RuleKind::MapRemote => {
                if let Some(target) = rule.target.as_deref().filter(|s| !s.is_empty()) {
                    if let Some(orig) = remap(parts, target) {
                        mapped_from = Some(orig);
                    }
                }
            }
        }
    }

    Outcome::Forward { mapped_from }
}

/// Point `parts` at `target`'s host/scheme, preserving the original path+query.
/// Returns the original host that was replaced. Fully effective for plain HTTP;
/// for already-tunneled HTTPS the upstream connection is fixed by CONNECT, so
/// only the Host header changes (documented limitation).
fn remap(parts: &mut Parts, target: &str) -> Option<String> {
    let target_uri: Uri = target.parse().ok()?;
    let target_authority = target_uri.authority()?.clone();
    let orig_host = parts
        .headers
        .get("host")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
        .or_else(|| parts.uri.host().map(|h| h.to_string()))?;

    let path_and_query = parts
        .uri
        .path_and_query()
        .cloned()
        .unwrap_or_else(|| "/".parse().unwrap());
    let scheme = target_uri
        .scheme()
        .cloned()
        .or_else(|| parts.uri.scheme().cloned())?;

    let new_uri = Uri::builder()
        .scheme(scheme)
        .authority(target_authority.clone())
        .path_and_query(path_and_query)
        .build()
        .ok()?;
    parts.uri = new_uri;

    if let Ok(hv) = HeaderValue::try_from(target_authority.as_str()) {
        parts.headers.insert("host", hv);
    }
    Some(orig_host)
}

/// Best-effort content type from a file extension, for Map Local responses.
pub fn guess_media_type(path: &str) -> &'static str {
    let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "json" => "application/json",
        "html" | "htm" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript",
        "css" => "text/css",
        "xml" => "application/xml",
        "txt" => "text/plain; charset=utf-8",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::glob_match;

    #[test]
    fn globs() {
        assert!(glob_match("*", "anything"));
        assert!(glob_match("https://api.example.com/*", "https://api.example.com/v1/users"));
        assert!(glob_match("*/graphql", "https://x.com/graphql"));
        assert!(glob_match("https://*.example.com/*", "https://api.example.com/a"));
        assert!(!glob_match("https://api.example.com/*", "https://other.com/x"));
        assert!(glob_match("exact", "exact"));
        assert!(!glob_match("exact", "exactly"));
    }
}
