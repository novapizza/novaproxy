//! Traffic-control rules: matching + application. Pure logic over `http` parts
//! and [`nova_proto::Rule`], so it stays unit-testable without a live proxy.

use hudsucker::hyper::header::{HeaderName, HeaderValue};
use hudsucker::hyper::http::request::Parts;
use hudsucker::hyper::Uri;
use nova_proto::{Fault, FaultMode, Rule, RuleHit, RuleKind};
use std::collections::HashMap;
use std::sync::Mutex;

/// The decision produced by evaluating the rule set against a request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Forward the (possibly-mutated) request. `mapped_from` is set if a
    /// Map Remote rule rewrote the destination, and `rewritten` when a Rewrite
    /// rule changed a header — the UI marks the flow as edited either way, and
    /// a header rewrite is otherwise invisible in the outcome.
    Forward { mapped_from: Option<String>, rewritten: bool },
    /// Short-circuit with a 403.
    Block,
    /// Short-circuit by serving a local file.
    MapLocal(String),
    /// Inject a failure. Request-phase modes answer instead of forwarding;
    /// response-phase ones (PassCut, CutBody, Delay) forward the (possibly
    /// edited) request and act on its response. `mapped_from` / `rewritten`
    /// as for `Forward`, from rules that ran before this one.
    Fault { rule_id: String, fault: Fault, mapped_from: Option<String>, rewritten: bool },
}

/// How many requests each rule acted on since the last reset. Lives in the
/// engine, not in `rules.json`: counts are about a test run, not the rule.
#[derive(Debug, Default)]
pub struct RuleHits(Mutex<HashMap<String, u32>>);

impl RuleHits {
    /// Count one action of `rule`, unless it already used up its `times`.
    /// Returns whether the rule may act.
    pub fn take(&self, rule: &Rule) -> bool {
        let mut map = self.0.lock().unwrap();
        let n = map.entry(rule.id.clone()).or_insert(0);
        if rule.times.is_some_and(|t| *n >= t) {
            return false;
        }
        *n += 1;
        true
    }

    pub fn get(&self, rule_id: &str) -> u32 {
        self.0.lock().unwrap().get(rule_id).copied().unwrap_or(0)
    }

    /// Every rule that acted at least once, by id.
    pub fn snapshot(&self) -> Vec<RuleHit> {
        let mut out: Vec<RuleHit> =
            self.0.lock().unwrap().iter().map(|(id, n)| RuleHit { rule_id: id.clone(), hits: *n }).collect();
        out.sort_by(|a, b| a.rule_id.cmp(&b.rule_id));
        out
    }

    /// Zero the given rules' counts (all rules when `ids` is `None`), which
    /// also re-arms rules limited by `times`.
    pub fn reset(&self, ids: Option<&[String]>) {
        let mut map = self.0.lock().unwrap();
        match ids {
            Some(ids) => ids.iter().for_each(|id| {
                map.remove(id);
            }),
            None => map.clear(),
        }
    }
}

/// What a request is matched on, beyond its URL.
pub struct MatchCtx<'a> {
    /// `scheme://host/path`, as for [`apply_request`].
    pub url: &'a str,
    /// Request method; an empty string matches only rules without a method.
    pub method: &'a str,
    /// Hit counter: counts actions and enforces `times`. `None` = neither.
    pub hits: Option<&'a RuleHits>,
    /// Host globs Fault rules may act on ([`nova_proto::FaultSettings`]).
    pub fault_hosts: &'a [String],
}

/// Whether a Fault rule may act on `host`: it must match one allowlist glob.
pub fn fault_host_allowed(allow: &[String], host: &str) -> bool {
    let host = host.trim_end_matches('.');
    !host.is_empty() && allow.iter().any(|g| !g.trim().is_empty() && glob_match(g.trim(), host))
}

fn method_matches(rule: &Rule, method: &str) -> bool {
    match rule.method.as_deref().map(str::trim) {
        None | Some("") | Some("*") => true,
        Some(m) => m.eq_ignore_ascii_case(method),
    }
}

/// Count an action when a counter is attached; without one every match acts.
fn take(ctx: &MatchCtx, rule: &Rule) -> bool {
    ctx.hits.is_none_or(|h| h.take(rule))
}

/// The Fault rule answering the CONNECT for `host` with 407, if any. CONNECT
/// carries no path, so patterns are tested against `https://host/`.
pub fn connect_fault(
    rules: &[Rule],
    host: &str,
    hits: Option<&RuleHits>,
    fault_hosts: &[String],
) -> Option<(String, Fault)> {
    let url = format!("https://{host}/");
    rules
        .iter()
        .filter(|r| r.enabled && r.kind == RuleKind::Fault)
        .filter(|r| r.fault.as_ref().is_some_and(|f| f.mode == FaultMode::ProxyAuth407))
        .filter(|r| method_matches(r, "CONNECT") && glob_match(&r.pattern, &url))
        .filter(|_| fault_host_allowed(fault_hosts, host))
        .find(|r| hits.is_none_or(|h| h.take(r)))
        .map(|r| (r.id.clone(), r.fault.clone().unwrap_or_default()))
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
    let method = parts.method.to_string();
    apply_request_with(rules, &MatchCtx { url, method: &method, hits: None, fault_hosts: &[] }, parts)
}

/// [`apply_request`] with method matching, hit counting and the Fault host
/// allowlist. Rules run in order; the first one that answers (Block, Map
/// Local, Fault) ends the pass, after the edits of the rules before it.
pub fn apply_request_with(rules: &[Rule], ctx: &MatchCtx, parts: &mut Parts) -> Outcome {
    let mut mapped_from: Option<String> = None;
    let mut rewritten = false;
    let url = ctx.url;

    for rule in rules.iter().filter(|r| r.enabled) {
        if !glob_match(&rule.pattern, url) || !method_matches(rule, ctx.method) {
            continue;
        }
        match rule.kind {
            RuleKind::Block => {
                if take(ctx, rule) {
                    return Outcome::Block;
                }
            }
            RuleKind::MapLocal => {
                if let Some(path) = rule.target.clone().filter(|s| !s.is_empty()) {
                    if take(ctx, rule) {
                        return Outcome::MapLocal(path);
                    }
                }
            }
            RuleKind::Fault => {
                let Some(fault) = rule.fault.clone() else { continue };
                let host = url.parse::<Uri>().ok().and_then(|u| u.host().map(str::to_string)).unwrap_or_default();
                if fault_host_allowed(ctx.fault_hosts, &host) && take(ctx, rule) {
                    return Outcome::Fault { rule_id: rule.id.clone(), fault, mapped_from, rewritten };
                }
            }
            RuleKind::Rewrite => {
                if let (Some(name), Some(value)) = (&rule.header_name, &rule.header_value) {
                    if let (Ok(n), Ok(v)) = (HeaderName::try_from(name.as_str()), HeaderValue::try_from(value.as_str()))
                    {
                        if !take(ctx, rule) {
                            continue;
                        }
                        // Only a header that actually changed counts as a
                        // rewrite: re-setting the value that was already there
                        // is not an edit the user needs told about.
                        let before = parts.headers.get(&n).cloned();
                        parts.headers.insert(n, v.clone());
                        rewritten = rewritten || before.as_ref() != Some(&v);
                    }
                }
            }
            RuleKind::MapRemote => {
                if let Some(target) = rule.target.as_deref().filter(|s| !s.is_empty()) {
                    if !take(ctx, rule) {
                        continue;
                    }
                    if let Some(orig) = remap(parts, target) {
                        mapped_from = Some(orig);
                    }
                }
            }
        }
    }

    Outcome::Forward { mapped_from, rewritten }
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

    let path_and_query = parts.uri.path_and_query().cloned().unwrap_or_else(|| "/".parse().unwrap());
    let scheme = target_uri.scheme().cloned().or_else(|| parts.uri.scheme().cloned())?;

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
        Request::builder().uri(url).header("host", host).body(()).unwrap().into_parts().0
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
            ..Default::default()
        }
    }

    #[test]
    fn no_rules_forwards_untouched() {
        let mut parts = parts_for("http://api.example.com/v1");
        assert_eq!(
            apply_request(&[], "http://api.example.com/v1", &mut parts),
            Outcome::Forward { mapped_from: None, rewritten: false }
        );
    }

    #[test]
    fn disabled_rule_is_ignored() {
        let mut r = rule(RuleKind::Block);
        r.enabled = false;
        let mut parts = parts_for("http://x.com/a");
        assert_eq!(
            apply_request(&[r], "http://x.com/a", &mut parts),
            Outcome::Forward { mapped_from: None, rewritten: false }
        );
    }

    #[test]
    fn non_matching_pattern_is_skipped() {
        let mut r = rule(RuleKind::Block);
        r.pattern = "https://blocked.com/*".into();
        let mut parts = parts_for("http://allowed.com/a");
        assert_eq!(
            apply_request(&[r], "http://allowed.com/a", &mut parts),
            Outcome::Forward { mapped_from: None, rewritten: false }
        );
    }

    #[test]
    fn block_short_circuits() {
        let mut r = rule(RuleKind::Block);
        r.pattern = "*ads*".into();
        let mut parts = parts_for("http://ads.example.com/track");
        assert_eq!(apply_request(&[r], "http://ads.example.com/track", &mut parts), Outcome::Block);
    }

    #[test]
    fn map_local_returns_target_path() {
        let mut r = rule(RuleKind::MapLocal);
        r.target = Some("/tmp/stub.json".into());
        let mut parts = parts_for("http://x.com/a");
        assert_eq!(apply_request(&[r], "http://x.com/a", &mut parts), Outcome::MapLocal("/tmp/stub.json".into()));
    }

    #[test]
    fn map_local_with_empty_target_falls_through() {
        let mut r = rule(RuleKind::MapLocal);
        r.target = Some(String::new());
        let mut parts = parts_for("http://x.com/a");
        assert_eq!(
            apply_request(&[r], "http://x.com/a", &mut parts),
            Outcome::Forward { mapped_from: None, rewritten: false }
        );
    }

    #[test]
    fn re_setting_the_same_header_is_not_an_edit() {
        // A rule that keeps a header at the value it already had changed
        // nothing, and a marker on that row would be noise on every request the
        // rule matches.
        let mut r = rule(RuleKind::Rewrite);
        r.header_name = Some("x-custom".into());
        r.header_value = Some("same".into());
        let mut parts = parts_for("http://x.com/a");
        parts.headers.insert(HeaderName::from_static("x-custom"), HeaderValue::from_static("same"));
        assert_eq!(
            apply_request(&[r], "http://x.com/a", &mut parts),
            Outcome::Forward { mapped_from: None, rewritten: false }
        );
    }

    #[test]
    fn rewrite_sets_header() {
        let mut r = rule(RuleKind::Rewrite);
        r.header_name = Some("x-custom".into());
        r.header_value = Some("novaproxy".into());
        let mut parts = parts_for("http://x.com/a");
        let outcome = apply_request(&[r], "http://x.com/a", &mut parts);
        // `rewritten` is what tells the UI this row was edited: a header rewrite
        // leaves no other trace in the outcome.
        assert_eq!(outcome, Outcome::Forward { mapped_from: None, rewritten: true });
        assert_eq!(parts.headers.get("x-custom").unwrap(), "novaproxy");
    }

    #[test]
    fn map_remote_rewrites_destination() {
        let mut r = rule(RuleKind::MapRemote);
        r.target = Some("https://staging.example.com".into());
        let mut parts = parts_for("http://api.example.com/v1/users?q=1");

        let outcome = apply_request(&[r], "http://api.example.com/v1/users?q=1", &mut parts);
        assert_eq!(outcome, Outcome::Forward { mapped_from: Some("api.example.com".into()), rewritten: false });
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
        assert_eq!(outcome, Outcome::Forward { mapped_from: None, rewritten: false });
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

    fn fault_rule(mode: FaultMode) -> Rule {
        Rule { kind: RuleKind::Fault, fault: Some(Fault { mode, ..Default::default() }), ..rule(RuleKind::Fault) }
    }

    fn ctx<'a>(url: &'a str, method: &'a str, hits: Option<&'a RuleHits>, allow: &'a [String]) -> MatchCtx<'a> {
        MatchCtx { url, method, hits, fault_hosts: allow }
    }

    #[test]
    fn method_narrows_the_match() {
        let mut r = rule(RuleKind::Block);
        r.method = Some("post".into());
        let mut parts = parts_for("http://x.com/a");
        let fwd = Outcome::Forward { mapped_from: None, rewritten: false };
        assert_eq!(apply_request_with(&[r.clone()], &ctx("http://x.com/a", "GET", None, &[]), &mut parts), fwd);
        assert_eq!(apply_request_with(&[r], &ctx("http://x.com/a", "POST", None, &[]), &mut parts), Outcome::Block);
    }

    #[test]
    fn times_limits_actions_and_reset_rearms() {
        let mut r = rule(RuleKind::Block);
        r.times = Some(1);
        let hits = RuleHits::default();
        let mut parts = parts_for("http://x.com/a");
        let c = ctx("http://x.com/a", "GET", Some(&hits), &[]);
        assert_eq!(apply_request_with(&[r.clone()], &c, &mut parts), Outcome::Block);
        assert_eq!(
            apply_request_with(&[r.clone()], &c, &mut parts),
            Outcome::Forward { mapped_from: None, rewritten: false },
            "first only: the second request passes"
        );
        assert_eq!(hits.get("r1"), 1);
        hits.reset(None);
        assert_eq!(apply_request_with(&[r], &c, &mut parts), Outcome::Block);
    }

    #[test]
    fn hits_count_every_action_without_a_limit() {
        let r = rule(RuleKind::Block);
        let hits = RuleHits::default();
        let mut parts = parts_for("http://x.com/a");
        for _ in 0..3 {
            apply_request_with(std::slice::from_ref(&r), &ctx("http://x.com/a", "GET", Some(&hits), &[]), &mut parts);
        }
        assert_eq!(hits.snapshot(), vec![RuleHit { rule_id: "r1".into(), hits: 3 }]);
    }

    #[test]
    fn fault_acts_only_on_allowed_hosts() {
        let r = fault_rule(FaultMode::DropBefore);
        let hits = RuleHits::default();
        let mut parts = parts_for("https://api.dev.example.com/v1");
        let url = "https://api.dev.example.com/v1";
        let fwd = Outcome::Forward { mapped_from: None, rewritten: false };
        assert_eq!(
            apply_request_with(std::slice::from_ref(&r), &ctx(url, "GET", Some(&hits), &[]), &mut parts),
            fwd,
            "empty allowlist"
        );
        let other = vec!["*.prod.example.com".to_string()];
        assert_eq!(
            apply_request_with(std::slice::from_ref(&r), &ctx(url, "GET", Some(&hits), &other), &mut parts),
            fwd
        );
        assert_eq!(hits.get("r1"), 0, "a rule that did not act is not counted");
        let allow = vec!["*.dev.example.com".to_string()];
        match apply_request_with(&[r], &ctx(url, "GET", Some(&hits), &allow), &mut parts) {
            Outcome::Fault { rule_id, fault, .. } => {
                assert_eq!(rule_id, "r1");
                assert_eq!(fault.mode, FaultMode::DropBefore);
            }
            other => panic!("expected a fault, got {other:?}"),
        }
    }

    #[test]
    fn fault_without_a_fault_body_does_nothing() {
        let r = Rule { fault: None, ..fault_rule(FaultMode::DropBefore) };
        let mut parts = parts_for("http://x.com/a");
        let allow = vec!["*".to_string()];
        assert_eq!(
            apply_request_with(&[r], &ctx("http://x.com/a", "GET", None, &allow), &mut parts),
            Outcome::Forward { mapped_from: None, rewritten: false }
        );
    }

    #[test]
    fn fault_keeps_the_edits_of_earlier_rules() {
        let mut rewrite = rule(RuleKind::Rewrite);
        rewrite.header_name = Some("x-a".into());
        rewrite.header_value = Some("1".into());
        let allow = vec!["x.com".to_string()];
        let mut parts = parts_for("http://x.com/a");
        let out = apply_request_with(
            &[rewrite, fault_rule(FaultMode::PassCut)],
            &ctx("http://x.com/a", "POST", None, &allow),
            &mut parts,
        );
        assert!(matches!(out, Outcome::Fault { rewritten: true, .. }), "{out:?}");
        assert_eq!(parts.headers.get("x-a").unwrap(), "1");
    }

    #[test]
    fn connect_407_matches_host_patterns() {
        let mut r = fault_rule(FaultMode::ProxyAuth407);
        r.pattern = "https://auth.example.com/*".into();
        let allow = vec!["auth.example.com".to_string()];
        assert!(connect_fault(&[r.clone()], "auth.example.com", None, &allow).is_some());
        assert!(connect_fault(&[r.clone()], "other.example.com", None, &allow).is_none());
        assert!(connect_fault(&[r.clone()], "auth.example.com", None, &[]).is_none(), "not allowed");
        let drop = fault_rule(FaultMode::DropBefore);
        assert!(connect_fault(&[drop], "auth.example.com", None, &allow).is_none(), "only 407 acts on CONNECT");
    }

    #[test]
    fn allowlist_globs() {
        let allow = vec!["*.example.com".to_string(), " exact.test ".to_string()];
        assert!(fault_host_allowed(&allow, "a.example.com"));
        assert!(fault_host_allowed(&allow, "exact.test"));
        assert!(!fault_host_allowed(&allow, "example.org"));
        assert!(!fault_host_allowed(&allow, ""));
        assert!(!fault_host_allowed(&["".to_string()], "x.com"), "a blank entry allows nothing");
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
