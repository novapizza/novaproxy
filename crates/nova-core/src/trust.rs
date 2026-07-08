//! OS trust-store integration. Installing a root CA always crosses one OS auth
//! gate — on macOS we trigger the native password dialog via `osascript`, never
//! asking the user to run terminal commands. macOS lands first (per the doc);
//! Windows/Linux are stubbed with a clear error until their variants ship.

use std::path::Path;
use std::process::Command;

use anyhow::{bail, Result};

/// Is our CA actually trusted by the OS as a root?
///
/// Presence in the keychain is NOT sufficient: a cert can sit in the System
/// keychain with no trust settings at all, in which case macOS still rejects
/// every leaf it signs (the browser shows unstyled pages / missing HTTPS
/// assets while the app thinks the CA is installed). We therefore require BOTH
/// that our cert is present (matched by fingerprint, so a stale same-name cert
/// doesn't fool us) AND that an admin-domain trust setting exists for it.
pub fn is_trusted(fingerprint: &str) -> bool {
    #[cfg(target_os = "macos")]
    {
        present_in_system_keychain(fingerprint) && has_admin_trust_setting()
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = fingerprint;
        false
    }
}

/// Our cert (by SHA-1 fingerprint) is in the System keychain.
#[cfg(target_os = "macos")]
fn present_in_system_keychain(fingerprint: &str) -> bool {
    let target = fingerprint.replace(':', "").to_ascii_uppercase();
    let out = Command::new("security")
        .args([
            "find-certificate",
            "-a",
            "-Z",
            "/Library/Keychains/System.keychain",
        ])
        .output();
    if let Ok(out) = out {
        let text = String::from_utf8_lossy(&out.stdout).to_ascii_uppercase();
        return text.contains(&target);
    }
    false
}

/// An admin-domain trust setting exists for our root. This is the domain
/// `install` writes to (`add-trusted-cert -d`); `dump-trust-settings -d` lists
/// exactly those, identifying each cert by common name. When the domain has no
/// trust settings the command prints to stderr and leaves stdout empty, so a
/// name match on stdout is a reliable "trusted" signal.
#[cfg(target_os = "macos")]
fn has_admin_trust_setting() -> bool {
    let out = Command::new("security")
        .args(["dump-trust-settings", "-d"])
        .output();
    if let Ok(out) = out {
        return String::from_utf8_lossy(&out.stdout).contains(CA_COMMON_NAME);
    }
    false
}

/// Common name of our root CA, used to delete it from the keychain by name.
#[cfg(target_os = "macos")]
const CA_COMMON_NAME: &str = "NovaProxy Root CA";

/// Install the CA into the system trust store behind one native auth prompt.
///
/// The result is decided by the *end state* (is the cert now in the store?),
/// not by `osascript`'s exit code — `security add-trusted-cert` can report a
/// nonzero status or emit stderr noise even when the cert lands, which showed up
/// as a spurious "install failed" while the cert was actually installed.
pub fn install(cert_path: &Path, fingerprint: &str) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        let ran = run_osascript(&install_script(cert_path));
        confirm(ran, is_trusted(fingerprint), "install")
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (cert_path, fingerprint);
        bail!("Automatic CA install is currently implemented for macOS only");
    }
}

/// Remove the CA from the system trust store AND the keychain. Confirmed by
/// end state: success means the cert is no longer present.
pub fn uninstall(cert_path: &Path, fingerprint: &str) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        let ran = run_osascript(&uninstall_script(cert_path));
        confirm(ran, !is_trusted(fingerprint), "uninstall")
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (cert_path, fingerprint);
        bail!("Automatic CA uninstall is currently implemented for macOS only");
    }
}

/// Reconcile the command's outcome with the observed end state. If the end
/// state is what we wanted, it's a success regardless of the exit code. If not,
/// surface the command's own error (e.g. the user cancelled the auth prompt),
/// falling back to a generic message when the command claimed success.
#[cfg(target_os = "macos")]
fn confirm(ran: Result<()>, reached_goal: bool, action: &str) -> Result<()> {
    if reached_goal {
        Ok(())
    } else if let Err(e) = ran {
        Err(e)
    } else {
        bail!("certificate {action} did not take effect")
    }
}

#[cfg(target_os = "macos")]
fn install_script(cert_path: &Path) -> String {
    let path = cert_path.to_string_lossy().replace('"', "\\\"");
    format!(
        "do shell script \"security add-trusted-cert -d -r trustRoot -k /Library/Keychains/System.keychain \\\"{path}\\\"\" with administrator privileges"
    )
}

#[cfg(target_os = "macos")]
fn uninstall_script(cert_path: &Path) -> String {
    let path = cert_path.to_string_lossy().replace('"', "\\\"");
    // Two steps behind ONE auth prompt: clear the admin trust setting, then
    // delete the cert from the System keychain. `remove-trusted-cert` only drops
    // trust settings — the certificate itself lingers in the keychain, so
    // `is_trusted` (a presence check) would keep reporting it as installed and
    // the removal would appear to do nothing. `;` (not `&&`) so the delete runs
    // even when there were no trust settings left to remove.
    format!(
        "do shell script \"security remove-trusted-cert -d \\\"{path}\\\" ; security delete-certificate -c \\\"{CA_COMMON_NAME}\\\" /Library/Keychains/System.keychain\" with administrator privileges"
    )
}

#[cfg(target_os = "macos")]
fn run_osascript(script: &str) -> Result<()> {
    let status = Command::new("osascript").args(["-e", script]).output()?;
    if status.status.success() {
        Ok(())
    } else {
        let err = String::from_utf8_lossy(&status.stderr);
        // User cancelling the auth dialog shows up as "User canceled." (-128).
        bail!("trust-store change failed: {}", err.trim());
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn install_script_adds_trusted_root_to_system_keychain() {
        let s = install_script(Path::new("/tmp/ca.pem"));
        assert!(s.contains("add-trusted-cert"));
        assert!(s.contains("-r trustRoot"));
        assert!(s.contains("/Library/Keychains/System.keychain"));
        assert!(s.contains("\\\"/tmp/ca.pem\\\""), "cert path is quoted for the shell");
        assert!(s.contains("with administrator privileges"));
    }

    #[test]
    fn uninstall_script_removes_trust_and_deletes_the_cert() {
        let s = uninstall_script(Path::new("/tmp/ca.pem"));
        // Regression: uninstall must ALSO delete the cert, not just its trust
        // settings — otherwise is_trusted (presence check) stays true forever.
        assert!(s.contains("remove-trusted-cert"), "clears trust settings");
        assert!(s.contains("delete-certificate"), "and deletes the lingering cert");
        assert!(s.contains(CA_COMMON_NAME), "deletes by our unique common name");
        // A single '; ' sequences the two so delete runs regardless of the first.
        assert!(s.contains(" ; security delete-certificate"));
        // One prompt for both.
        assert_eq!(s.matches("with administrator privileges").count(), 1);
    }

    #[test]
    fn scripts_escape_quotes_in_the_cert_path() {
        let s = uninstall_script(Path::new("/tmp/we\"ird/ca.pem"));
        assert!(s.contains("we\\\"ird"), "embedded quote is escaped");
    }

    #[test]
    fn confirm_trusts_end_state_over_a_failing_exit_code() {
        // The command "failed" but the cert is actually present → success.
        // This is the spurious-failure case the user hit on install.
        assert!(confirm(Err(anyhow::anyhow!("nonzero exit")), true, "install").is_ok());
    }

    #[test]
    fn confirm_reports_command_error_when_goal_not_reached() {
        // Goal not reached and the command errored (e.g. user cancelled the
        // prompt) → surface that specific error.
        let e = confirm(Err(anyhow::anyhow!("User canceled.")), false, "install").unwrap_err();
        assert!(e.to_string().contains("User canceled."));
    }

    #[test]
    fn confirm_reports_generic_error_when_command_lied_about_success() {
        // Command claimed success but the end state disagrees.
        let e = confirm(Ok(()), false, "uninstall").unwrap_err();
        assert!(e.to_string().contains("uninstall did not take effect"));
    }

    #[test]
    fn confirm_ok_when_goal_reached_and_command_ok() {
        assert!(confirm(Ok(()), true, "install").is_ok());
    }
}
