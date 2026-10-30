//! The connections we make *upstream*, to origin servers: which certificates
//! they trust, and how their failures are described.
//!
//! Trust is Mozilla's root set **plus** the operating system's store. The
//! Mozilla set alone failed on any machine whose security software re-signs
//! HTTPS — corporate firewalls, antivirus "web shields", Zscaler and the like
//! install their own root in the OS store, browsers trust it, and NovaProxy
//! answered every such site with a bare 502. NovaProxy's own CA is in that
//! store too (it has to be, for interception to work) and is left out: the
//! engine must never accept a certificate it could have minted itself.

use std::sync::{Arc, OnceLock};

use anyhow::Result;
use hudsucker::rustls::crypto::aws_lc_rs;
use hudsucker::rustls::pki_types::CertificateDer;
use hudsucker::rustls::{ClientConfig, RootCertStore};

use crate::ca::CaMaterial;

/// Bytes every NovaProxy root carries in its subject. Older roots from a
/// regenerated CA can still sit in the OS store, and they are just as much
/// ours as the current one.
const OWN_CA_NAME: &[u8] = b"NovaProxy Root CA";

/// TLS configuration for upstream connections. Unrelated to the CA presented
/// downstream, except that this one must not trust it.
pub fn client_tls_config(ca: &CaMaterial) -> Result<Arc<ClientConfig>> {
    let roots = roots(ca.cert_der().as_deref(), native_roots());
    let config = ClientConfig::builder_with_provider(Arc::new(aws_lc_rs::default_provider()))
        .with_safe_default_protocol_versions()?
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(Arc::new(config))
}

/// The OS trust store, read once per process. Reading it is slow on some
/// machines (the macOS keychain, a large Windows store) and it does not change
/// between proxy restarts often enough to matter.
fn native_roots() -> &'static [CertificateDer<'static>] {
    static ROOTS: OnceLock<Vec<CertificateDer<'static>>> = OnceLock::new();
    ROOTS.get_or_init(|| {
        let started = std::time::Instant::now();
        let loaded = rustls_native_certs::load_native_certs();
        if !loaded.errors.is_empty() {
            tracing::warn!(
                errors = loaded.errors.len(),
                first = %loaded.errors[0],
                "some OS trust store certificates could not be read"
            );
        }
        tracing::info!(
            certs = loaded.certs.len(),
            ms = started.elapsed().as_millis() as u64,
            "loaded OS trust store for upstream TLS"
        );
        loaded.certs
    })
}

/// Mozilla's roots plus `native`, minus anything that is a NovaProxy root.
fn roots(own_ca: Option<&[u8]>, native: &[CertificateDer<'static>]) -> RootCertStore {
    let mut store = RootCertStore::empty();
    store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let mut skipped = 0;
    for cert in native {
        let der: &[u8] = cert.as_ref();
        if own_ca == Some(der) || contains(der, OWN_CA_NAME) {
            skipped += 1;
            continue;
        }
        // An OS store can hold certificates rustls cannot use as anchors
        // (unsupported keys, malformed entries); those are skipped, not fatal.
        let _ = store.add(cert.clone());
    }
    if skipped > 0 {
        tracing::debug!(skipped, "left NovaProxy's own CA out of upstream trust");
    }
    store
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// An error and every cause beneath it, as one line.
///
/// hyper's client error displays as `client error (Connect)` whatever went
/// wrong; the useful part — the DNS failure, the refused connection, the
/// certificate rustls rejected — is in its `source` chain.
pub fn describe(err: &(dyn std::error::Error + 'static)) -> String {
    let mut parts = vec![err.to_string()];
    let mut source = err.source();
    while let Some(e) = source {
        let s = e.to_string();
        // Some layers repeat their cause in their own message.
        if !parts.last().is_some_and(|p| p.contains(&s)) {
            parts.push(s);
        }
        source = e.source();
    }
    parts.join(": ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cert(name: &str) -> CertificateDer<'static> {
        let key = rcgen::KeyPair::generate().unwrap();
        let mut params = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap();
        params.distinguished_name.push(rcgen::DnType::CommonName, name);
        params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        params.self_signed(&key).unwrap().der().clone()
    }

    #[test]
    fn os_roots_are_trusted_alongside_mozillas() {
        let corporate = cert("Example Corp TLS Inspection CA");
        let base = roots(None, &[]).len();
        let store = roots(None, std::slice::from_ref(&corporate));
        assert!(base > 100, "Mozilla's roots are there: {base}");
        assert_eq!(store.len(), base + 1);
    }

    #[test]
    fn novaproxys_own_roots_are_never_trusted_upstream() {
        let ours = cert("NovaProxy Root CA");
        let older = cert("NovaProxy Root CA");
        let base = roots(None, &[]).len();
        let store = roots(Some(ours.as_ref()), &[ours.clone(), older]);
        assert_eq!(store.len(), base, "neither the current nor an older NovaProxy root");
    }

    #[test]
    fn the_machines_own_store_loads_and_builds_a_config() {
        let dir = std::env::temp_dir().join(format!("novaproxy-upstream-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let ca = CaMaterial::load_or_create(&dir).unwrap();
        client_tls_config(&ca).expect("upstream TLS config builds");
        eprintln!("os roots: {}", native_roots().len());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[derive(Debug)]
    struct Layer(&'static str, Option<Box<Layer>>);
    impl std::fmt::Display for Layer {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(self.0)
        }
    }
    impl std::error::Error for Layer {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            self.1.as_deref().map(|e| e as _)
        }
    }

    #[test]
    fn describe_walks_the_whole_cause_chain() {
        let err = Layer(
            "client error (Connect)",
            Some(Box::new(Layer(
                "invalid peer certificate: UnknownIssuer",
                Some(Box::new(Layer("UnknownIssuer", None))),
            ))),
        );
        assert_eq!(describe(&err), "client error (Connect): invalid peer certificate: UnknownIssuer");
    }
}
