//! SSL-proxying scope: decides, per host, whether NovaProxy decrypts a
//! CONNECT (MITM) or tunnels it through untouched. Pure logic over
//! [`nova_proto::TlsScope`] so it stays unit-testable without a live proxy.

use nova_proto::TlsScope;

use crate::rules::glob_match;

/// Whether `host` should be decrypted under `scope`.
///
/// * `intercept_all`: decrypt unless `host` matches an `exclude` glob.
/// * otherwise: decrypt only if `host` matches an `include` glob.
///
/// Globs are matched against the bare host (no port), e.g. `*.apple.com`.
pub fn should_intercept_host(scope: &TlsScope, host: &str) -> bool {
    if scope.intercept_all {
        !scope.exclude.iter().any(|p| glob_match(p, host))
    } else {
        scope.include.iter().any(|p| glob_match(p, host))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope(intercept_all: bool, include: &[&str], exclude: &[&str]) -> TlsScope {
        TlsScope {
            intercept_all,
            include: include.iter().map(|s| s.to_string()).collect(),
            exclude: exclude.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn default_scope_intercepts_everything() {
        let s = TlsScope::default();
        assert!(should_intercept_host(&s, "api.example.com"));
        assert!(should_intercept_host(&s, "anything.at.all"));
    }

    #[test]
    fn intercept_all_tunnels_excluded_hosts() {
        let s = scope(true, &[], &["*.apple.com", "pinned.example.com"]);
        assert!(!should_intercept_host(&s, "gateway.apple.com"), "matches *.apple.com → tunnel");
        assert!(!should_intercept_host(&s, "pinned.example.com"));
        assert!(should_intercept_host(&s, "api.example.com"), "not excluded → decrypt");
    }

    #[test]
    fn allowlist_mode_decrypts_only_included_hosts() {
        let s = scope(false, &["api.example.com", "*.mysite.dev"], &[]);
        assert!(should_intercept_host(&s, "api.example.com"));
        assert!(should_intercept_host(&s, "staging.mysite.dev"));
        assert!(!should_intercept_host(&s, "google.com"), "not on the list → tunnel");
    }

    #[test]
    fn allowlist_mode_with_empty_list_tunnels_all() {
        let s = scope(false, &[], &[]);
        assert!(!should_intercept_host(&s, "api.example.com"));
    }

    #[test]
    fn exclude_is_case_insensitive_on_host() {
        let s = scope(true, &[], &["*.APPLE.com"]);
        assert!(!should_intercept_host(&s, "gateway.apple.com"));
    }
}
