//! OS trust-store integration. Installing a root CA always crosses one OS auth
//! gate — on macOS we trigger the native password dialog via `osascript`, never
//! asking the user to run terminal commands. macOS lands first (per the doc);
//! Windows/Linux are stubbed with a clear error until their variants ship.

use std::path::Path;
use std::process::Command;

use anyhow::{bail, Result};

/// Is our CA currently present in the system trust store?
pub fn is_trusted(fingerprint: &str) -> bool {
    #[cfg(target_os = "macos")]
    {
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
    #[cfg(not(target_os = "macos"))]
    {
        let _ = fingerprint;
        false
    }
}

/// Install the CA into the system trust store behind one native auth prompt.
pub fn install(cert_path: &Path) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        let path = cert_path.to_string_lossy().replace('"', "\\\"");
        let script = format!(
            "do shell script \"security add-trusted-cert -d -r trustRoot -k /Library/Keychains/System.keychain \\\"{path}\\\"\" with administrator privileges"
        );
        run_osascript(&script)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = cert_path;
        bail!("Automatic CA install is currently implemented for macOS only");
    }
}

/// Remove the CA from the system trust store.
pub fn uninstall(cert_path: &Path) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        let path = cert_path.to_string_lossy().replace('"', "\\\"");
        // remove-trusted-cert needs admin to touch the admin trust settings.
        let script = format!(
            "do shell script \"security remove-trusted-cert -d \\\"{path}\\\"\" with administrator privileges"
        );
        run_osascript(&script)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = cert_path;
        bail!("Automatic CA uninstall is currently implemented for macOS only");
    }
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
