//! OS trust-store integration for macOS, Windows and Linux.
//!
//! Installing a root CA always crosses one OS auth gate, but *which* gate is a
//! choice. NovaProxy defaults to the **user domain** on every platform, which
//! needs no administrator rights:
//!
//! | Platform | User domain (default) | All users (opt-in) |
//! |---|---|---|
//! | macOS | login keychain, `add-trusted-cert` without `-d` (keychain dialog) | System keychain, `-d`, one `osascript` admin prompt |
//! | Windows | `certutil -user -addstore ROOT` (no prompt at all) | `certutil -addstore ROOT` behind one UAC prompt |
//! | Linux | NSS databases — Chrome/Chromium plus every Firefox profile (no prompt) | `/usr/local/share/ca-certificates` or `/etc/pki/…/anchors` + `update-ca-*`, one `pkexec` prompt |
//!
//! **Linux's user domain is browsers only.** Linux has no per-user OpenSSL trust
//! store, so `curl`, Python, Go and friends only honour the CA once it is
//! installed for all users. That is a real platform difference, surfaced in the
//! UI rather than papered over.
//!
//! Because both domains can hold the cert (e.g. a machine that installed
//! system-wide before the default changed), trust is reported per domain — see
//! [`TrustState`] — and uninstall clears every domain the cert is in.
//!
//! Command *plans* are pure data (see [`nova_os::oscmd`]) so every platform's
//! behaviour is unit-tested from any machine; only execution is platform-gated.

use std::path::{Path, PathBuf};

use anyhow::{bail, Result};

use nova_os::oscmd::{capture, have_tool, Elevation, Plan, Step};

/// Common name of our root CA. Also the NSS nickname and the anchor file stem.
pub const CA_COMMON_NAME: &str = "NovaProxy Root CA";
/// File name used for the CA inside a Linux system anchor directory.
const LINUX_ANCHOR_FILE: &str = "novaproxy-root-ca.crt";

/// Everything the trust stores need to identify our CA.
///
/// Two digests because the platforms disagree: macOS and NSS print SHA-256,
/// Windows names a certificate by its SHA-1 thumbprint. Carrying both lets every
/// platform match *our exact certificate* rather than "something with that name",
/// which is what stops a leftover cert from a regenerated CA reading as trusted.
#[derive(Debug, Clone)]
pub struct CaId {
    pub cert_path: PathBuf,
    /// SHA-256 fingerprint, colon-separated uppercase hex.
    pub sha256: String,
    /// SHA-1 thumbprint, colon-separated uppercase hex.
    pub sha1: String,
}

impl CaId {
    pub fn new(cert_path: PathBuf, sha256: String, sha1: String) -> Self {
        Self { cert_path, sha256, sha1 }
    }

    /// Identity of a loaded CA.
    pub fn of(ca: &crate::ca::CaMaterial) -> Self {
        Self {
            cert_path: ca.cert_path.clone(),
            sha256: ca.fingerprint(),
            sha1: ca.thumbprint_sha1(),
        }
    }

    /// Identity of the certificate at `cert_path`, read from disk.
    pub fn from_path(cert_path: &Path) -> Result<Self> {
        let pem = std::fs::read_to_string(cert_path)
            .map_err(|e| anyhow::anyhow!("cannot read {}: {e}", cert_path.display()))?;
        let sha256 = crate::ca::fingerprint_pem(&pem)
            .ok_or_else(|| anyhow::anyhow!("{} is not a PEM certificate", cert_path.display()))?;
        let sha1 = crate::ca::thumbprint_sha1_pem(&pem).unwrap_or_default();
        Ok(Self { cert_path: cert_path.to_path_buf(), sha256, sha1 })
    }
}

/// Which OS trust domain an install targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustDomain {
    /// Just this user — no administrator rights.
    User,
    /// Every user on the machine — requires elevation.
    System,
}

/// Where our CA is currently trusted. Four reachable states: user, system,
/// both, neither.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TrustState {
    pub user: bool,
    pub system: bool,
}

impl TrustState {
    /// Trusted in at least one domain — i.e. HTTPS interception works for at
    /// least some of this user's apps.
    pub fn any(&self) -> bool {
        self.user || self.system
    }
}

/// Where our CA is trusted, per domain.
///
/// Presence alone is not enough on macOS: a cert can sit in a keychain with no
/// trust settings, in which case macOS still rejects every leaf it signs (the
/// browser shows unstyled pages while the app believes the CA is installed).
/// Each platform's check is written to answer "would a TLS handshake succeed",
/// not "is a file somewhere".
pub fn trust_state(ca: &CaId) -> TrustState {
    #[cfg(target_os = "macos")]
    {
        macos::trust_state(ca)
    }
    #[cfg(target_os = "windows")]
    {
        windows::trust_state(ca)
    }
    #[cfg(target_os = "linux")]
    {
        linux::trust_state(ca)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        let _ = ca;
        TrustState::default()
    }
}

/// Is our CA trusted by the OS as a root in *any* domain?
pub fn is_trusted(ca: &CaId) -> bool {
    trust_state(ca).any()
}

/// Install the CA into `domain` behind at most one native auth prompt.
///
/// The result is decided by the *end state* (is the cert now trusted in that
/// domain?), not by the command's exit code — `security add-trusted-cert` and
/// `certutil` both emit noise or non-zero statuses on success, which showed up as
/// a spurious "install failed" while the cert was actually installed.
pub fn install(ca: &CaId, domain: TrustDomain) -> Result<()> {
    let plan = install_plan(&ca.cert_path, domain)?;
    let ran = plan.run();
    let reached = match domain {
        TrustDomain::User => trust_state(ca).user,
        TrustDomain::System => trust_state(ca).system,
    };
    confirm(ran, reached, "install")
}

/// Remove the CA from every trust domain that holds it, confirmed by end state.
///
/// Each domain needing work costs its own prompt (a keychain dialog, a UAC
/// prompt, a polkit prompt) — there is no way to batch across domains.
pub fn uninstall(ca: &CaId) -> Result<()> {
    let state = present_state(ca);
    if !state.any() {
        return Ok(()); // nothing installed anywhere
    }
    let mut ran = Ok(());
    if state.user {
        ran = ran.and(uninstall_plan(ca, TrustDomain::User)?.run());
    }
    if state.system {
        ran = ran.and(uninstall_plan(ca, TrustDomain::System)?.run());
    }
    confirm(ran, !is_trusted(ca), "uninstall")
}

/// Which domains hold the cert *at all* (not necessarily trusted).
///
/// Uninstall keys off presence rather than trust so a cert sitting in a store
/// without trust settings — and a machine that predates the user-domain default
/// — still gets cleaned up.
fn present_state(ca: &CaId) -> TrustState {
    #[cfg(target_os = "macos")]
    {
        macos::present_state(ca)
    }
    #[cfg(not(target_os = "macos"))]
    {
        // Windows and Linux have no "present but untrusted" state for a root:
        // being in the store (or the anchor dir) *is* the trust.
        trust_state(ca)
    }
}

/// The commands that install the CA into `domain` on the current platform.
fn install_plan(cert_path: &Path, domain: TrustDomain) -> Result<Plan> {
    #[cfg(target_os = "macos")]
    {
        macos::install_plan(cert_path, domain)
    }
    #[cfg(target_os = "windows")]
    {
        Ok(windows::install_plan(cert_path, domain))
    }
    #[cfg(target_os = "linux")]
    {
        linux::install_plan(cert_path, domain)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        let _ = (cert_path, domain);
        bail!("Automatic CA install is not implemented for this platform");
    }
}

fn uninstall_plan(ca: &CaId, domain: TrustDomain) -> Result<Plan> {
    #[cfg(target_os = "macos")]
    {
        macos::uninstall_plan(&ca.cert_path, domain)
    }
    #[cfg(target_os = "windows")]
    {
        Ok(windows::uninstall_plan(&ca.sha1, domain))
    }
    #[cfg(target_os = "linux")]
    {
        let _ = ca;
        linux::uninstall_plan(domain)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        let _ = (ca, domain);
        bail!("Automatic CA uninstall is not implemented for this platform");
    }
}

/// Reconcile a command's outcome with the observed end state. If the end state is
/// what we wanted, it is a success regardless of the exit code. If not, surface
/// the command's own error (e.g. the user cancelled the prompt), falling back to
/// a generic message when the command claimed success.
fn confirm(ran: Result<()>, reached_goal: bool, action: &str) -> Result<()> {
    if reached_goal {
        Ok(())
    } else if let Err(e) = ran {
        Err(e)
    } else {
        bail!("certificate {action} did not take effect")
    }
}

/// Normalise a fingerprint or a store dump to a bare lowercase hex string, so a
/// digest can be found regardless of how the tool spaced or cased it.
///
/// `certutil` prints `Cert Hash(sha1): 1a 2b 3c …`, `security` prints
/// `AA BB CC …`, and our own fingerprints are colon-separated. Reducing both
/// sides to hex makes one comparison work everywhere. Surrounding labels leave
/// stray hex letters behind, which is harmless: needles are 40–64 hex characters
/// long, far beyond what label noise can spell.
pub fn hex_only(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_ascii_hexdigit())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

/// Does `haystack` (a tool's output) contain this certificate digest?
pub fn contains_digest(haystack: &str, digest: &str) -> bool {
    let needle = hex_only(digest);
    !needle.is_empty() && hex_only(haystack).contains(&needle)
}

/* ================================== macOS ================================== */

pub mod macos {
    use super::*;

    pub const SYSTEM_KEYCHAIN: &str = "/Library/Keychains/System.keychain";

    /// Absolute path of the current user's login keychain.
    pub fn login_keychain() -> Option<PathBuf> {
        std::env::var_os("HOME").map(|home| {
            PathBuf::from(home)
                .join("Library/Keychains")
                .join("login.keychain-db")
        })
    }

    /// `add-trusted-cert` args. The user domain omits `-d`, which is exactly what
    /// makes it work without an administrator password.
    pub fn install_args(cert_path: &Path, keychain: &Path, domain: TrustDomain) -> Vec<String> {
        let mut args = vec!["add-trusted-cert".to_string()];
        if domain == TrustDomain::System {
            args.push("-d".to_string());
        }
        args.push("-r".to_string());
        args.push("trustRoot".to_string());
        args.push("-k".to_string());
        args.push(keychain.display().to_string());
        args.push(cert_path.display().to_string());
        args
    }

    /// Clearing trust settings only; the cert itself must be deleted separately.
    pub fn remove_trust_args(cert_path: &Path, domain: TrustDomain) -> Vec<String> {
        let mut args = vec!["remove-trusted-cert".to_string()];
        if domain == TrustDomain::System {
            args.push("-d".to_string());
        }
        args.push(cert_path.display().to_string());
        args
    }

    /// `remove-trusted-cert` leaves the certificate in the keychain, so a
    /// presence check would keep reporting it as installed. Delete it too.
    pub fn delete_cert_args(keychain: &Path) -> Vec<String> {
        vec![
            "delete-certificate".to_string(),
            "-c".to_string(),
            CA_COMMON_NAME.to_string(),
            keychain.display().to_string(),
        ]
    }

    /// `dump-trust-settings` lists exactly one domain: no flag for the user
    /// domain, `-d` for admin. Getting this backwards would report the wrong
    /// domain as trusted.
    pub fn dump_trust_args(domain: TrustDomain) -> Vec<String> {
        match domain {
            TrustDomain::User => vec!["dump-trust-settings".to_string()],
            TrustDomain::System => vec!["dump-trust-settings".to_string(), "-d".to_string()],
        }
    }

    pub fn keychain_for(domain: TrustDomain) -> Result<PathBuf> {
        match domain {
            TrustDomain::User => login_keychain()
                .ok_or_else(|| anyhow::anyhow!("cannot locate the login keychain ($HOME unset)")),
            TrustDomain::System => Ok(PathBuf::from(SYSTEM_KEYCHAIN)),
        }
    }

    pub fn install_plan(cert_path: &Path, domain: TrustDomain) -> Result<Plan> {
        let keychain = keychain_for(domain)?;
        let steps = vec![Step::with_args(
            "security",
            install_args(cert_path, &keychain, domain),
        )];
        Ok(Plan::new(steps, elevation(domain)))
    }

    pub fn uninstall_plan(cert_path: &Path, domain: TrustDomain) -> Result<Plan> {
        let keychain = keychain_for(domain)?;
        // Best-effort: a cert with no trust settings left still has to be
        // deleted, so the second step must run even if the first fails.
        Ok(Plan::best_effort(
            vec![
                Step::with_args("security", remove_trust_args(cert_path, domain)),
                Step::with_args("security", delete_cert_args(&keychain)),
            ],
            elevation(domain),
        ))
    }

    /// The user domain raises the ordinary keychain dialog (no admin); the system
    /// domain needs the admin password.
    pub fn elevation(domain: TrustDomain) -> Elevation {
        match domain {
            TrustDomain::User => Elevation::None,
            TrustDomain::System => Elevation::MacAdmin,
        }
    }

    /// A trust setting for our root exists in this domain. When a domain has no
    /// trust settings the command prints to stderr and leaves stdout empty, so a
    /// name match is a reliable signal.
    pub fn trust_settings_name_present(output: &str) -> bool {
        output.contains(CA_COMMON_NAME)
    }

    pub fn trust_state(ca: &CaId) -> TrustState {
        TrustState {
            user: present(&ca.sha256, TrustDomain::User) && has_trust_setting(TrustDomain::User),
            system: present(&ca.sha256, TrustDomain::System)
                && has_trust_setting(TrustDomain::System),
        }
    }

    pub fn present_state(ca: &CaId) -> TrustState {
        TrustState {
            user: present(&ca.sha256, TrustDomain::User),
            system: present(&ca.sha256, TrustDomain::System),
        }
    }

    fn present(fingerprint: &str, domain: TrustDomain) -> bool {
        let Ok(keychain) = keychain_for(domain) else { return false };
        let out = capture(
            "security",
            &["find-certificate", "-a", "-Z", &keychain.display().to_string()],
        );
        contains_digest(&out, fingerprint)
    }

    fn has_trust_setting(domain: TrustDomain) -> bool {
        let args = dump_trust_args(domain);
        let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        trust_settings_name_present(&capture("security", &refs))
    }
}

/* ================================= Windows ================================= */

pub mod windows {
    use super::*;

    /// Windows keeps trusted roots in the `ROOT` store, per user or per machine.
    pub const ROOT_STORE: &str = "ROOT";

    /// `certutil [-user] -addstore -f ROOT <cert>`.
    ///
    /// `-user` targets the current user's store, which needs no elevation at all
    /// — not even a prompt. Without it, `certutil` writes the machine store and
    /// requires administrator rights.
    pub fn install_args(cert_path: &Path, domain: TrustDomain) -> Vec<String> {
        let mut args = Vec::new();
        if domain == TrustDomain::User {
            args.push("-user".to_string());
        }
        args.extend([
            "-addstore".to_string(),
            "-f".to_string(),
            ROOT_STORE.to_string(),
            cert_path.display().to_string(),
        ]);
        args
    }

    /// `certutil [-user] -delstore ROOT <sha1 thumbprint>`.
    ///
    /// Deleting by thumbprint rather than by name so a same-named cert from a
    /// previous CA generation is never removed by accident — and so ours is
    /// always found even if the display name differs.
    pub fn uninstall_args(thumbprint: &str, domain: TrustDomain) -> Vec<String> {
        let mut args = Vec::new();
        if domain == TrustDomain::User {
            args.push("-user".to_string());
        }
        args.extend([
            "-delstore".to_string(),
            ROOT_STORE.to_string(),
            hex_only(thumbprint),
        ]);
        args
    }

    /// `certutil [-user] -store ROOT` — dumps the store for a presence check.
    pub fn store_query_args(domain: TrustDomain) -> Vec<String> {
        let mut args = Vec::new();
        if domain == TrustDomain::User {
            args.push("-user".to_string());
        }
        args.extend(["-store".to_string(), ROOT_STORE.to_string()]);
        args
    }

    /// The user store needs no elevation; the machine store needs one UAC prompt.
    pub fn elevation(domain: TrustDomain) -> Elevation {
        match domain {
            TrustDomain::User => Elevation::None,
            TrustDomain::System => Elevation::WindowsUac,
        }
    }

    pub fn install_plan(cert_path: &Path, domain: TrustDomain) -> Plan {
        Plan::new(
            vec![Step::with_args("certutil", install_args(cert_path, domain))],
            elevation(domain),
        )
    }

    pub fn uninstall_plan(thumbprint: &str, domain: TrustDomain) -> Plan {
        Plan::new(
            vec![Step::with_args("certutil", uninstall_args(thumbprint, domain))],
            elevation(domain),
        )
    }

    /// On Windows, being in the ROOT store *is* the trust — there is no separate
    /// trust-settings layer to check, unlike macOS.
    ///
    /// `certutil -store` identifies certs by SHA-1 thumbprint, so that is what we
    /// look for; the SHA-256 fingerprint is also tried because newer builds print
    /// both and matching either is still an exact match on our certificate.
    pub fn trust_state(ca: &CaId) -> TrustState {
        let matches = |domain: TrustDomain| {
            let args = store_query_args(domain);
            let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
            let out = capture("certutil", &refs);
            contains_digest(&out, &ca.sha1) || contains_digest(&out, &ca.sha256)
        };
        TrustState {
            user: matches(TrustDomain::User),
            system: matches(TrustDomain::System),
        }
    }
}

/* ================================== Linux ================================== */

pub mod linux {
    use super::*;

    /// Where a distro family keeps extra trust anchors and how it rebuilds the
    /// bundle afterwards.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct AnchorLayout {
        pub dir: PathBuf,
        pub update_command: &'static str,
        pub family: &'static str,
    }

    impl AnchorLayout {
        pub fn anchor_path(&self) -> PathBuf {
            self.dir.join(LINUX_ANCHOR_FILE)
        }
    }

    /// Anchor layouts in probe order. Debian/Ubuntu first because their
    /// directory only exists on that family, then the `ca-trust` families.
    pub fn known_layouts() -> Vec<AnchorLayout> {
        vec![
            AnchorLayout {
                dir: PathBuf::from("/usr/local/share/ca-certificates"),
                update_command: "update-ca-certificates",
                family: "debian",
            },
            AnchorLayout {
                dir: PathBuf::from("/etc/pki/ca-trust/source/anchors"),
                update_command: "update-ca-trust",
                family: "fedora",
            },
            AnchorLayout {
                dir: PathBuf::from("/etc/ca-certificates/trust-source/anchors"),
                update_command: "update-ca-trust",
                family: "arch",
            },
        ]
    }

    /// Pick the layout whose anchor directory exists, using `exists` to probe.
    /// Injectable so the choice is testable without those directories present.
    pub fn detect_layout_with<F: Fn(&Path) -> bool>(exists: F) -> Option<AnchorLayout> {
        known_layouts().into_iter().find(|l| exists(&l.dir))
    }

    pub fn detect_layout() -> Option<AnchorLayout> {
        detect_layout_with(|p| p.is_dir())
    }

    /// Copy the CA into the anchor directory and rebuild the system bundle, both
    /// behind one polkit prompt.
    pub fn system_install_plan(cert_path: &Path, layout: &AnchorLayout) -> Plan {
        Plan::new(
            vec![
                Step::with_args(
                    "install",
                    vec![
                        "-m".to_string(),
                        "644".to_string(),
                        cert_path.display().to_string(),
                        layout.anchor_path().display().to_string(),
                    ],
                ),
                Step::new(layout.update_command, &[]),
            ],
            Elevation::LinuxPkexec,
        )
    }

    pub fn system_uninstall_plan(layout: &AnchorLayout) -> Plan {
        // Best-effort: rebuilding the bundle must happen even if the anchor was
        // already gone, or the CA stays trusted until the next rebuild.
        Plan::best_effort(
            vec![
                Step::with_args(
                    "rm",
                    vec!["-f".to_string(), layout.anchor_path().display().to_string()],
                ),
                Step::new(layout.update_command, &[]),
            ],
            Elevation::LinuxPkexec,
        )
    }

    /// NSS database paths to install into: Chrome/Chromium's shared DB plus every
    /// Firefox profile that has one.
    ///
    /// Linux has no per-user OpenSSL trust store, so this — browsers only — is
    /// the whole of the Linux user domain.
    pub fn nss_databases(home: &Path) -> Vec<String> {
        let mut dbs = Vec::new();
        let chromium = home.join(".pki/nssdb");
        if chromium.is_dir() {
            dbs.push(format!("sql:{}", chromium.display()));
        }
        dbs.extend(firefox_profiles(home).into_iter().map(|p| format!("sql:{}", p.display())));
        dbs
    }

    /// Firefox profile directories that already contain an NSS database.
    ///
    /// Firefox keeps one database per profile, so a CA has to be added to each.
    /// Profiles without a `cert9.db` have never been launched; writing there
    /// would create a database Firefox then ignores.
    pub fn firefox_profiles(home: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        for root in [
            home.join(".mozilla/firefox"),
            // Snap and Flatpak installs keep profiles elsewhere.
            home.join("snap/firefox/common/.mozilla/firefox"),
            home.join(".var/app/org.mozilla.firefox/.mozilla/firefox"),
        ] {
            let Ok(entries) = std::fs::read_dir(&root) else { continue };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.join("cert9.db").is_file() {
                    out.push(path);
                }
            }
        }
        out.sort();
        out
    }

    /// `certutil -d <db> -A -t C,, -n "<name>" -i <cert>` — add a trusted CA.
    ///
    /// `C,,` marks it trusted for TLS server authentication only, which is all a
    /// debugging proxy needs.
    pub fn nss_add_args(db: &str, cert_path: &Path) -> Vec<String> {
        vec![
            "-d".to_string(),
            db.to_string(),
            "-A".to_string(),
            "-t".to_string(),
            "C,,".to_string(),
            "-n".to_string(),
            CA_COMMON_NAME.to_string(),
            "-i".to_string(),
            cert_path.display().to_string(),
        ]
    }

    pub fn nss_delete_args(db: &str) -> Vec<String> {
        vec![
            "-d".to_string(),
            db.to_string(),
            "-D".to_string(),
            "-n".to_string(),
            CA_COMMON_NAME.to_string(),
        ]
    }

    /// `certutil -d <db> -L -n "<name>"` — print our cert, for a presence check.
    pub fn nss_query_args(db: &str) -> Vec<String> {
        vec![
            "-d".to_string(),
            db.to_string(),
            "-L".to_string(),
            "-n".to_string(),
            CA_COMMON_NAME.to_string(),
        ]
    }

    /// Add the CA to every NSS database found. No elevation: these are the user's
    /// own files.
    pub fn user_install_plan(cert_path: &Path, dbs: &[String]) -> Plan {
        Plan::best_effort(
            dbs.iter()
                .map(|db| Step::with_args("certutil", nss_add_args(db, cert_path)))
                .collect(),
            Elevation::None,
        )
    }

    pub fn user_uninstall_plan(dbs: &[String]) -> Plan {
        Plan::best_effort(
            dbs.iter()
                .map(|db| Step::with_args("certutil", nss_delete_args(db)))
                .collect(),
            Elevation::None,
        )
    }

    fn home() -> Option<PathBuf> {
        std::env::var_os("HOME").map(PathBuf::from)
    }

    pub fn install_plan(cert_path: &Path, domain: TrustDomain) -> Result<Plan> {
        match domain {
            TrustDomain::User => {
                if !have_tool("certutil") {
                    bail!(
                        "NSS `certutil` is not installed, so the CA cannot be added to browser \
                         trust stores. Install it (Debian/Ubuntu: libnss3-tools, Fedora: nss-tools, \
                         Arch: nss) or choose \"install for all users\" instead."
                    );
                }
                let home = home().ok_or_else(|| anyhow::anyhow!("$HOME is not set"))?;
                let dbs = nss_databases(&home);
                if dbs.is_empty() {
                    bail!(
                        "no browser trust databases found under {}. Launch Chrome or Firefox once, \
                         or choose \"install for all users\".",
                        home.display()
                    );
                }
                Ok(user_install_plan(cert_path, &dbs))
            }
            TrustDomain::System => {
                let layout = detect_layout().ok_or_else(|| {
                    anyhow::anyhow!(
                        "unrecognised distribution: none of the known trust-anchor directories \
                         exist (Debian/Ubuntu, Fedora/RHEL, Arch)"
                    )
                })?;
                Ok(system_install_plan(cert_path, &layout))
            }
        }
    }

    pub fn uninstall_plan(domain: TrustDomain) -> Result<Plan> {
        match domain {
            TrustDomain::User => {
                let home = home().ok_or_else(|| anyhow::anyhow!("$HOME is not set"))?;
                Ok(user_uninstall_plan(&nss_databases(&home)))
            }
            TrustDomain::System => {
                let layout = detect_layout()
                    .ok_or_else(|| anyhow::anyhow!("unrecognised distribution"))?;
                Ok(system_uninstall_plan(&layout))
            }
        }
    }

    /// Is the installed anchor *our* certificate? Compared by fingerprint, so a
    /// leftover anchor from a regenerated CA does not read as installed.
    pub fn anchor_is_ours(anchor_pem: &str, fingerprint: &str) -> bool {
        crate::ca::fingerprint_pem(anchor_pem)
            .map(|fp| hex_only(&fp) == hex_only(fingerprint))
            .unwrap_or(false)
    }

    pub fn trust_state(ca: &CaId) -> TrustState {
        let fingerprint = ca.sha256.as_str();
        let system = detect_layout()
            .and_then(|l| std::fs::read_to_string(l.anchor_path()).ok())
            .map(|pem| anchor_is_ours(&pem, fingerprint))
            .unwrap_or(false);

        let user = home()
            .map(|home| {
                let dbs = nss_databases(&home);
                dbs.iter().any(|db| {
                    let args = nss_query_args(db);
                    let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
                    let out = capture("certutil", &refs);
                    // `-L -n` prints the certificate when present and an error
                    // when not; matching the fingerprint avoids trusting a
                    // same-nicknamed cert from an older CA.
                    contains_digest(&out, fingerprint)
                })
            })
            .unwrap_or(false);

        TrustState { user, system }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args_of(v: &[String]) -> Vec<&str> {
        v.iter().map(|s| s.as_str()).collect()
    }

    /* ---------------------------- digest matching ---------------------------- */

    #[test]
    fn digests_match_across_tool_formats() {
        let ours = "1A:2B:3C:4D";
        // macOS `security` spaces the bytes; Windows `certutil` lowercases them.
        assert!(contains_digest("Cert Hash(sha1): 1a 2b 3c 4d", ours));
        assert!(contains_digest("SHA-256 hash: 1A2B3C4D", ours));
        assert!(contains_digest("...\n  1a:2b:3c:4d  \n...", ours));
        assert!(!contains_digest("Cert Hash(sha1): 99 88 77 66", ours));
    }

    #[test]
    fn an_empty_digest_never_matches() {
        // A CA whose fingerprint could not be computed must not be reported as
        // trusted just because some hex exists in the output.
        assert!(!contains_digest("1a2b3c", ""));
        assert!(!contains_digest("", "1a2b"));
    }

    #[test]
    fn hex_only_strips_formatting() {
        assert_eq!(hex_only("AA:BB:cc dd\n"), "aabbccdd");
        // Only hex characters survive. Labels do contribute stray hex letters
        // ("Cert Hash(sha1)" leaves "ceaa1"), which is harmless: a needle is a
        // 40- or 64-character digest, so label noise cannot fabricate a match.
        assert_eq!(hex_only("Cert Hash(sha1): 0F"), "ceaa10f");
    }

    /* -------------------------------- macOS -------------------------------- */

    #[test]
    fn macos_user_install_omits_the_admin_domain_flag() {
        let args = macos::install_args(
            Path::new("/tmp/ca.pem"),
            Path::new("/Users/me/Library/Keychains/login.keychain-db"),
            TrustDomain::User,
        );
        let args = args_of(&args);
        assert_eq!(args[0], "add-trusted-cert");
        assert!(args.contains(&"trustRoot"));
        assert!(args.contains(&"/Users/me/Library/Keychains/login.keychain-db"));
        assert_eq!(*args.last().unwrap(), "/tmp/ca.pem");
        assert!(!args.contains(&"-d"), "the user domain must not pass -d");
        assert_eq!(macos::elevation(TrustDomain::User), Elevation::None);
    }

    #[test]
    fn macos_system_install_uses_the_admin_domain_and_one_prompt() {
        let args = macos::install_args(
            Path::new("/tmp/ca.pem"),
            Path::new(macos::SYSTEM_KEYCHAIN),
            TrustDomain::System,
        );
        let args = args_of(&args);
        assert!(args.contains(&"-d"));
        assert!(args.contains(&macos::SYSTEM_KEYCHAIN));
        assert_eq!(macos::elevation(TrustDomain::System), Elevation::MacAdmin);
    }

    #[test]
    fn macos_uninstall_clears_trust_then_deletes_the_cert() {
        let clear = macos::remove_trust_args(Path::new("/tmp/ca.pem"), TrustDomain::User);
        assert_eq!(args_of(&clear), vec!["remove-trusted-cert", "/tmp/ca.pem"]);
        let clear_admin = macos::remove_trust_args(Path::new("/tmp/ca.pem"), TrustDomain::System);
        assert!(args_of(&clear_admin).contains(&"-d"));

        // Regression: without the delete, a presence check stays true forever.
        let del = macos::delete_cert_args(Path::new("/kc/login.keychain-db"));
        let del = args_of(&del);
        assert_eq!(del[0], "delete-certificate");
        assert!(del.contains(&CA_COMMON_NAME));
        assert_eq!(*del.last().unwrap(), "/kc/login.keychain-db");
    }

    #[test]
    fn macos_dump_trust_settings_selects_the_domain() {
        assert_eq!(
            args_of(&macos::dump_trust_args(TrustDomain::User)),
            vec!["dump-trust-settings"]
        );
        assert_eq!(
            args_of(&macos::dump_trust_args(TrustDomain::System)),
            vec!["dump-trust-settings", "-d"]
        );
    }

    #[test]
    fn macos_uninstall_plan_attempts_both_steps() {
        let plan = macos::uninstall_plan(Path::new("/tmp/ca.pem"), TrustDomain::User).unwrap();
        assert_eq!(plan.steps.len(), 2);
        assert!(plan.best_effort, "the delete must run even if there was no trust setting");
    }

    #[test]
    fn macos_trust_settings_are_matched_by_our_common_name() {
        assert!(macos::trust_settings_name_present("... NovaProxy Root CA ..."));
        assert!(!macos::trust_settings_name_present("Some Other CA"));
    }

    /* ------------------------------- Windows ------------------------------- */

    #[test]
    fn windows_user_install_needs_no_elevation() {
        let args = windows::install_args(Path::new(r"C:\ca.pem"), TrustDomain::User);
        assert_eq!(
            args_of(&args),
            vec!["-user", "-addstore", "-f", "ROOT", r"C:\ca.pem"]
        );
        assert_eq!(
            windows::elevation(TrustDomain::User),
            Elevation::None,
            "the per-user ROOT store needs no UAC prompt at all"
        );
    }

    #[test]
    fn windows_machine_install_drops_user_and_takes_one_uac_prompt() {
        let args = windows::install_args(Path::new(r"C:\ca.pem"), TrustDomain::System);
        assert_eq!(args_of(&args), vec!["-addstore", "-f", "ROOT", r"C:\ca.pem"]);
        assert_eq!(windows::elevation(TrustDomain::System), Elevation::WindowsUac);
    }

    #[test]
    fn windows_uninstall_targets_the_thumbprint_not_the_name() {
        // By thumbprint so a same-named cert from an earlier CA generation is
        // never deleted by accident.
        let args = windows::uninstall_args("1A:2B:3C", TrustDomain::User);
        assert_eq!(args_of(&args), vec!["-user", "-delstore", "ROOT", "1a2b3c"]);
        let machine = windows::uninstall_args("1A2B3C", TrustDomain::System);
        assert_eq!(args_of(&machine), vec!["-delstore", "ROOT", "1a2b3c"]);
    }

    #[test]
    fn windows_store_query_selects_the_store() {
        assert_eq!(
            args_of(&windows::store_query_args(TrustDomain::User)),
            vec!["-user", "-store", "ROOT"]
        );
        assert_eq!(
            args_of(&windows::store_query_args(TrustDomain::System)),
            vec!["-store", "ROOT"]
        );
    }

    #[test]
    fn windows_uninstall_plan_is_a_single_command_too() {
        let plan = windows::uninstall_plan("1A:2B", TrustDomain::System);
        assert_eq!(plan.steps.len(), 1);
        assert_eq!(plan.elevation, Elevation::WindowsUac);
    }

    #[test]
    fn windows_install_plan_is_a_single_command() {
        // Windows can only elevate one process, so an elevated plan must have
        // exactly one step (see oscmd::windows_uac_step).
        let plan = windows::install_plan(Path::new(r"C:\ca.pem"), TrustDomain::System);
        assert_eq!(plan.steps.len(), 1);
        assert_eq!(plan.steps[0].program, "certutil");
    }

    /* -------------------------------- Linux -------------------------------- */

    #[test]
    fn linux_layout_detection_prefers_the_family_that_exists() {
        let debian = linux::detect_layout_with(|p| p == Path::new("/usr/local/share/ca-certificates"))
            .expect("debian layout");
        assert_eq!(debian.family, "debian");
        assert_eq!(debian.update_command, "update-ca-certificates");

        let fedora = linux::detect_layout_with(|p| p == Path::new("/etc/pki/ca-trust/source/anchors"))
            .expect("fedora layout");
        assert_eq!(fedora.family, "fedora");
        assert_eq!(fedora.update_command, "update-ca-trust");

        let arch =
            linux::detect_layout_with(|p| p == Path::new("/etc/ca-certificates/trust-source/anchors"))
                .expect("arch layout");
        assert_eq!(arch.family, "arch");

        assert!(
            linux::detect_layout_with(|_| false).is_none(),
            "an unrecognised distro must be reported, not guessed at"
        );
    }

    #[test]
    fn linux_anchor_path_is_inside_the_family_directory() {
        let layout = linux::known_layouts().remove(0);
        assert_eq!(
            layout.anchor_path(),
            Path::new("/usr/local/share/ca-certificates/novaproxy-root-ca.crt")
        );
        assert!(
            layout.anchor_path().extension().unwrap() == "crt",
            "update-ca-certificates only picks up .crt files"
        );
    }

    #[test]
    fn linux_system_install_copies_then_rebuilds_the_bundle_in_one_prompt() {
        let layout = linux::known_layouts().remove(0);
        let plan = linux::system_install_plan(Path::new("/tmp/ca.pem"), &layout);
        assert_eq!(plan.elevation, Elevation::LinuxPkexec);
        assert_eq!(plan.steps.len(), 2);
        assert_eq!(plan.steps[0].program, "install");
        assert!(plan.steps[0].args.contains(&"644".to_string()), "world-readable anchor");
        assert_eq!(plan.steps[1].program, "update-ca-certificates");
    }

    #[test]
    fn linux_system_uninstall_rebuilds_even_if_the_anchor_was_gone() {
        let layout = linux::known_layouts().remove(0);
        let plan = linux::system_uninstall_plan(&layout);
        assert!(plan.best_effort);
        assert_eq!(plan.steps[0].program, "rm");
        assert_eq!(plan.steps[1].program, "update-ca-certificates");
    }

    #[test]
    fn linux_nss_add_marks_the_cert_as_a_tls_ca() {
        let args = linux::nss_add_args("sql:/home/me/.pki/nssdb", Path::new("/tmp/ca.pem"));
        let args = args_of(&args);
        assert_eq!(args[0], "-d");
        assert_eq!(args[1], "sql:/home/me/.pki/nssdb");
        assert!(args.contains(&"-A"));
        assert!(args.contains(&"C,,"), "trusted for TLS server auth only");
        assert!(args.contains(&CA_COMMON_NAME));
        assert_eq!(*args.last().unwrap(), "/tmp/ca.pem");
    }

    #[test]
    fn linux_nss_delete_and_query_target_our_nickname() {
        assert_eq!(
            args_of(&linux::nss_delete_args("sql:/db")),
            vec!["-d", "sql:/db", "-D", "-n", CA_COMMON_NAME]
        );
        assert_eq!(
            args_of(&linux::nss_query_args("sql:/db")),
            vec!["-d", "sql:/db", "-L", "-n", CA_COMMON_NAME]
        );
    }

    #[test]
    fn linux_user_plan_writes_every_database_and_needs_no_root() {
        let dbs = vec!["sql:/a".to_string(), "sql:/b".to_string()];
        let plan = linux::user_install_plan(Path::new("/tmp/ca.pem"), &dbs);
        assert_eq!(plan.elevation, Elevation::None, "NSS databases are the user's own files");
        assert_eq!(plan.steps.len(), 2);
        assert!(
            plan.best_effort,
            "one locked profile must not stop the others from being trusted"
        );
    }

    #[test]
    fn linux_finds_chromium_and_firefox_databases() {
        // A fake $HOME: Chromium's shared DB, one launched Firefox profile, and
        // one profile that has never been opened.
        let home = std::env::temp_dir().join(format!("novaproxy-nss-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(home.join(".pki/nssdb")).unwrap();
        let launched = home.join(".mozilla/firefox/abc.default-release");
        std::fs::create_dir_all(&launched).unwrap();
        std::fs::write(launched.join("cert9.db"), b"x").unwrap();
        std::fs::create_dir_all(home.join(".mozilla/firefox/never-opened")).unwrap();

        let dbs = linux::nss_databases(&home);
        assert!(dbs.iter().any(|d| d.ends_with(".pki/nssdb")), "Chrome/Chromium: {dbs:?}");
        assert!(
            dbs.iter().any(|d| d.contains("abc.default-release")),
            "Firefox profiles each need their own database: {dbs:?}"
        );
        assert!(
            !dbs.iter().any(|d| d.contains("never-opened")),
            "a profile with no cert9.db has never run; writing there does nothing"
        );
        assert!(dbs.iter().all(|d| d.starts_with("sql:")), "NSS wants the sql: prefix");
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn linux_no_databases_is_an_empty_list_not_a_guess() {
        let home = std::env::temp_dir().join(format!("novaproxy-nss-empty-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).unwrap();
        assert!(linux::nss_databases(&home).is_empty());
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn linux_anchor_is_matched_by_fingerprint_not_by_existence() {
        // Build a real cert so the fingerprint is real.
        let dir = std::env::temp_dir().join(format!("novaproxy-anchor-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let ca = crate::ca::CaMaterial::load_or_create(&dir).unwrap();
        let fp = ca.fingerprint();

        assert!(linux::anchor_is_ours(&ca.cert_pem, &fp));
        // A leftover anchor from a regenerated CA must not read as installed.
        let other_dir = dir.join("other");
        let other = crate::ca::CaMaterial::load_or_create(&other_dir).unwrap();
        assert!(!linux::anchor_is_ours(&other.cert_pem, &fp));
        assert!(!linux::anchor_is_ours("not a certificate", &fp));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /* ------------------------------- shared ------------------------------- */

    #[test]
    fn trust_state_any_covers_all_four_states() {
        assert!(!TrustState { user: false, system: false }.any());
        assert!(TrustState { user: true, system: false }.any());
        assert!(TrustState { user: false, system: true }.any());
        assert!(TrustState { user: true, system: true }.any());
    }

    #[test]
    fn confirm_trusts_end_state_over_a_failing_exit_code() {
        // The command "failed" but the cert is actually trusted → success. This is
        // the spurious-failure case users hit on install.
        assert!(confirm(Err(anyhow::anyhow!("nonzero exit")), true, "install").is_ok());
    }

    #[test]
    fn confirm_reports_command_error_when_goal_not_reached() {
        let e = confirm(Err(anyhow::anyhow!("User canceled.")), false, "install").unwrap_err();
        assert!(e.to_string().contains("User canceled."));
    }

    #[test]
    fn confirm_reports_generic_error_when_command_lied_about_success() {
        let e = confirm(Ok(()), false, "uninstall").unwrap_err();
        assert!(e.to_string().contains("uninstall did not take effect"));
    }

    #[test]
    fn confirm_ok_when_goal_reached_and_command_ok() {
        assert!(confirm(Ok(()), true, "install").is_ok());
    }
}
