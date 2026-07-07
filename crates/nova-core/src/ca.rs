//! Root CA lifecycle: generate once, persist with tight permissions, and hand
//! a [`RcgenAuthority`] to hudsucker so it can mint per-host leaf certs on the
//! fly. The private key never leaves disk via any UI path.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use hudsucker::certificate_authority::RcgenAuthority;
use hudsucker::rustls::crypto::aws_lc_rs;
use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, DnType, IsCa, Issuer, KeyPair,
    KeyUsagePurpose,
};
use sha2::{Digest, Sha256};

/// Certs held in memory; mirrors the two files on disk.
pub struct CaMaterial {
    pub cert_pem: String,
    pub key_pem: String,
    pub cert_path: PathBuf,
}

impl CaMaterial {
    /// Load `ca.pem` + `ca.key` from `dir`, generating them on first run.
    pub fn load_or_create(dir: &Path) -> Result<Self> {
        fs::create_dir_all(dir).with_context(|| format!("create CA dir {}", dir.display()))?;
        let cert_path = dir.join("ca.pem");
        let key_path = dir.join("ca.key");

        if cert_path.exists() && key_path.exists() {
            let cert_pem = fs::read_to_string(&cert_path)?;
            let key_pem = fs::read_to_string(&key_path)?;
            return Ok(Self { cert_pem, key_pem, cert_path });
        }

        let (cert_pem, key_pem) = generate()?;
        fs::write(&cert_path, &cert_pem)?;
        write_private(&key_path, &key_pem)?;
        Ok(Self { cert_pem, key_pem, cert_path })
    }

    /// Build the hudsucker authority used to sign leaf certs during interception.
    pub fn authority(&self) -> Result<RcgenAuthority> {
        let key_pair = KeyPair::from_pem(&self.key_pem).context("parse CA private key")?;
        let issuer =
            Issuer::from_ca_cert_pem(&self.cert_pem, key_pair).context("parse CA certificate")?;
        // 1_000-entry leaf cache: minting per connection is a known perf trap.
        Ok(RcgenAuthority::new(issuer, 1_000, aws_lc_rs::default_provider()))
    }

    pub fn fingerprint(&self) -> String {
        fingerprint_pem(&self.cert_pem).unwrap_or_default()
    }

    pub fn subject(&self) -> String {
        "NovaProxy Root CA".to_string()
    }
}

/// Mint a fresh long-lived root CA (ECDSA P-256).
fn generate() -> Result<(String, String)> {
    let mut params = CertificateParams::new(Vec::new())?;
    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, "NovaProxy Root CA");
    dn.push(DnType::OrganizationName, "NovaProxy");
    params.distinguished_name = dn;
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.key_usages = vec![
        KeyUsagePurpose::KeyCertSign,
        KeyUsagePurpose::CrlSign,
        KeyUsagePurpose::DigitalSignature,
    ];
    // Long validity: this is a locally-trusted development root.
    params.not_before = time::OffsetDateTime::now_utc() - time::Duration::days(1);
    params.not_after = time::OffsetDateTime::now_utc() + time::Duration::days(3650);

    let key_pair = KeyPair::generate()?;
    let cert = params.self_signed(&key_pair)?;
    Ok((cert.pem(), key_pair.serialize_pem()))
}

/// Write a file readable/writable only by the owner (0600 on unix).
fn write_private(path: &Path, contents: &str) -> Result<()> {
    fs::write(path, contents)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

/// SHA-256 fingerprint of the DER inside a PEM certificate, as `AA:BB:...`.
pub fn fingerprint_pem(cert_pem: &str) -> Option<String> {
    let der = pem_to_der(cert_pem)?;
    let digest = Sha256::digest(&der);
    Some(
        digest
            .iter()
            .map(|b| format!("{b:02X}"))
            .collect::<Vec<_>>()
            .join(":"),
    )
}

fn pem_to_der(pem: &str) -> Option<Vec<u8>> {
    use base64::Engine;
    let body: String = pem
        .lines()
        .skip_while(|l| !l.starts_with("-----BEGIN"))
        .skip(1)
        .take_while(|l| !l.starts_with("-----END"))
        .collect();
    base64::engine::general_purpose::STANDARD.decode(body.trim()).ok()
}
