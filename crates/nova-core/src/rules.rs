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
    use super::*;
    use hudsucker::hyper::Request;

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

    #[test]
    fn globs_edge_cases() {
        // Empty pattern only matches empty text.
        assert!(glob_match("", ""));
        assert!(!glob_match("", "x"));
        // Bare "*" matches empty.
        assert!(glob_match("*", ""));
        // Leading + trailing wildcards (substring match).
        assert!(glob_match("*graphql*", "https://x.com/graphql?a=1"));
        assert!(!glob_match("*graphql*", "https://x.com/rest"));
        // Case-insensitive.
        assert!(glob_match("HTTPS://API.EXAMPLE.COM/*", "https://api.example.com/v1"));
        // Multiple interior segments, matched in order.
        assert!(glob_match("a*b*c", "axxbyyc"));
        assert!(!glob_match("a*b*c", "axxcbyy"));
    }

    /// Build request `Parts` for `url`, mirroring how the proxy reconstructs a
    /// request (absolute URI + Host header).
    fn parts_for(url: &str) -> Parts {
        let host = url.parse::<Uri>().unwrap().host().unwrap().to_string();
        Request::builder()
            .uri(url)
            .header("host", host)
            .body(())
            .unwrap()
            .into_parts()
            .0
    }

    fn rule(kind: RuleKind) -> Rule {
        Rule {
            id: "r1".into(),
            enabled: true,
            kind,
            name: "test".into(),
            pattern: "*".into(),
            target: None,
            header_name: None,
            header_value: None,
        }
    }

    #[test]
    fn no_rules_forwards_untouched() {
        let mut parts = parts_for("http://api.example.com/v1");
        assert_eq!(
            apply_request(&[], "http://api.example.com/v1", &mut parts),
            Outcome::Forward { mapped_from: None }
        );
    }

    #[test]
    fn disabled_rule_is_ignored() {
        let mut r = rule(RuleKind::Block);
        r.enabled = false;
        let mut parts = parts_for("http://x.com/a");
        assert_eq!(
            apply_request(&[r], "http://x.com/a", &mut parts),
            Outcome::Forward { mapped_from: None }
        );
    }

    #[test]
    fn non_matching_pattern_is_skipped() {
        let mut r = rule(RuleKind::Block);
        r.pattern = "https://blocked.com/*".into();
        let mut parts = parts_for("http://allowed.com/a");
        assert_eq!(
            apply_request(&[r], "http://allowed.com/a", &mut parts),
            Outcome::Forward { mapped_from: None }
        );
    }

    #[test]
    fn block_short_circuits() {
        let mut r = rule(RuleKind::Block);
        r.pattern = "*ads*".into();
        let mut parts = parts_for("http://ads.example.com/track");
        assert_eq!(
            apply_request(&[r], "http://ads.example.com/track", &mut parts),
            Outcome::Block
        );
    }

    #[test]
    fn map_local_returns_target_path() {
        let mut r = rule(RuleKind::MapLocal);
        r.target = Some("/tmp/stub.json".into());
        let mut parts = parts_for("http://x.com/a");
        assert_eq!(
            apply_request(&[r], "http://x.com/a", &mut parts),
            Outcome::MapLocal("/tmp/stub.json".into())
        );
    }

    #[test]
    fn map_local_with_empty_target_falls_through() {
        let mut r = rule(RuleKind::MapLocal);
        r.target = Some(String::new());
        let mut parts = parts_for("http://x.com/a");
        assert_eq!(
            apply_request(&[r], "http://x.com/a", &mut parts),
            Outcome::Forward { mapped_from: None }
        );
    }

    #[test]
    fn rewrite_sets_header() {
        let mut r = rule(RuleKind::Rewrite);
        r.header_name = Some("x-custom".into());
        r.header_value = Some("novaproxy".into());
        let mut parts = parts_for("http://x.com/a");
        let outcome = apply_request(&[r], "http://x.com/a", &mut parts);
        assert_eq!(outcome, Outcome::Forward { mapped_from: None });
        assert_eq!(parts.headers.get("x-custom").unwrap(), "novaproxy");
    }

    #[test]
    fn map_remote_rewrites_destination() {
        let mut r = rule(RuleKind::MapRemote);
        r.target = Some("https://staging.example.com".into());
        let mut parts = parts_for("http://api.example.com/v1/users?q=1");

        let outcome = apply_request(&[r], "http://api.example.com/v1/users?q=1", &mut parts);
        assert_eq!(
            outcome,
            Outcome::Forward { mapped_from: Some("api.example.com".into()) }
        );
        // Host + scheme point at the target; path/query are preserved.
        assert_eq!(parts.uri.host(), Some("staging.example.com"));
        assert_eq!(parts.uri.scheme_str(), Some("https"));
        assert_eq!(parts.uri.path(), "/v1/users");
        assert_eq!(parts.uri.query(), Some("q=1"));
        assert_eq!(parts.headers.get("host").unwrap(), "staging.example.com");
    }

    #[test]
    fn map_remote_with_authorityless_target_is_ignored() {
        // A path-only target has no authority: remap bails, the request forwards
        // untouched, and mapped_from stays None. (Note: a bare word like "host"
        // is parsed by hyper as an authority, so it would remap — use a path.)
        let mut r = rule(RuleKind::MapRemote);
        r.target = Some("/local/only".into());
        let mut parts = parts_for("http://api.example.com/v1");
        let outcome = apply_request(&[r], "http://api.example.com/v1", &mut parts);
        assert_eq!(outcome, Outcome::Forward { mapped_from: None });
        assert_eq!(parts.uri.host(), Some("api.example.com"));
    }

    #[test]
    fn first_terminal_rule_wins_but_earlier_edits_apply() {
        // A Rewrite runs, then Block short-circuits before a later MapLocal.
        let mut rewrite = rule(RuleKind::Rewrite);
        rewrite.header_name = Some("x-a".into());
        rewrite.header_value = Some("1".into());
        let block = rule(RuleKind::Block);
        let mut map_local = rule(RuleKind::MapLocal);
        map_local.target = Some("/never.json".into());

        let mut parts = parts_for("http://x.com/a");
        let outcome = apply_request(&[rewrite, block, map_local], "http://x.com/a", &mut parts);
        assert_eq!(outcome, Outcome::Block);
        // The earlier rewrite still applied before the block short-circuited.
        assert_eq!(parts.headers.get("x-a").unwrap(), "1");
    }

    #[test]
    fn media_types() {
        assert_eq!(guess_media_type("a.json"), "application/json");
        assert_eq!(guess_media_type("index.html"), "text/html; charset=utf-8");
        assert_eq!(guess_media_type("app.js"), "text/javascript");
        assert_eq!(guess_media_type("logo.PNG"), "image/png"); // case-insensitive
        assert_eq!(guess_media_type("photo.jpeg"), "image/jpeg");
        assert_eq!(guess_media_type("noext"), "application/octet-stream");
        assert_eq!(guess_media_type("weird.xyz"), "application/octet-stream");
    }
}
