//! System-proxy control (macOS). Routing all traffic through NovaProxy means
//! flipping the OS proxy for every network service. We **snapshot** the prior
//! per-service state first so it can be restored on disable — or on next launch
//! after a crash — and never leave the user without working internet.
//!
//! All mutating calls are batched into a single `osascript … with administrator
//! privileges` invocation, so the user sees exactly one auth prompt.

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

/// Saved proxy state for one network service.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceBackup {
    pub service: String,
    pub web_enabled: bool,
    pub web_host: String,
    pub web_port: String,
    pub secure_enabled: bool,
    pub secure_host: String,
    pub secure_port: String,
}

/// A snapshot of every service's proxy state, persisted for safe restore.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Backup {
    pub services: Vec<ServiceBackup>,
}

#[cfg(target_os = "macos")]
mod imp {
    use super::*;
    use std::process::Command;

    fn services() -> Vec<String> {
        let out = Command::new("networksetup")
            .arg("-listallnetworkservices")
            .output();
        let Ok(out) = out else { return Vec::new() };
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .skip(1) // header line
            .filter(|l| !l.starts_with('*')) // '*' = disabled service
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect()
    }

    fn get_proxy(flag: &str, service: &str) -> (bool, String, String) {
        let out = Command::new("networksetup")
            .args([flag, service])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default();
        let mut enabled = false;
        let mut host = String::new();
        let mut port = String::new();
        for line in out.lines() {
            if let Some(v) = line.strip_prefix("Enabled: ") {
                enabled = v.trim().eq_ignore_ascii_case("yes");
            } else if let Some(v) = line.strip_prefix("Server: ") {
                host = v.trim().to_string();
            } else if let Some(v) = line.strip_prefix("Port: ") {
                port = v.trim().to_string();
            }
        }
        (enabled, host, port)
    }

    pub fn snapshot() -> Backup {
        let mut services_out = Vec::new();
        for service in services() {
            let (we, wh, wp) = get_proxy("-getwebproxy", &service);
            let (se, sh, sp) = get_proxy("-getsecurewebproxy", &service);
            services_out.push(ServiceBackup {
                service,
                web_enabled: we,
                web_host: wh,
                web_port: wp,
                secure_enabled: se,
                secure_host: sh,
                secure_port: sp,
            });
        }
        Backup { services: services_out }
    }

    fn q(s: &str) -> String {
        // quote for the inner double-quoted shell string inside osascript
        format!("\\\"{}\\\"", s.replace('"', ""))
    }

    pub fn enable(host: &str, port: u16, backup: &Backup) -> Result<()> {
        let mut cmds = Vec::new();
        for s in &backup.services {
            let svc = q(&s.service);
            cmds.push(format!("networksetup -setwebproxy {svc} {host} {port}"));
            cmds.push(format!("networksetup -setsecurewebproxy {svc} {host} {port}"));
            cmds.push(format!("networksetup -setwebproxystate {svc} on"));
            cmds.push(format!("networksetup -setsecurewebproxystate {svc} on"));
        }
        run_admin(&cmds.join(" && "))
    }

    pub fn disable(backup: &Backup) -> Result<()> {
        let mut cmds = Vec::new();
        for s in &backup.services {
            let svc = q(&s.service);
            // Restore web proxy
            if s.web_enabled && !s.web_host.is_empty() {
                cmds.push(format!(
                    "networksetup -setwebproxy {svc} {} {}",
                    s.web_host, s.web_port
                ));
                cmds.push(format!("networksetup -setwebproxystate {svc} on"));
            } else {
                cmds.push(format!("networksetup -setwebproxystate {svc} off"));
            }
            // Restore secure proxy
            if s.secure_enabled && !s.secure_host.is_empty() {
                cmds.push(format!(
                    "networksetup -setsecurewebproxy {svc} {} {}",
                    s.secure_host, s.secure_port
                ));
                cmds.push(format!("networksetup -setsecurewebproxystate {svc} on"));
            } else {
                cmds.push(format!("networksetup -setsecurewebproxystate {svc} off"));
            }
        }
        run_admin(&cmds.join(" && "))
    }

    fn run_admin(shell: &str) -> Result<()> {
        let script = format!("do shell script \"{shell}\" with administrator privileges");
        let out = Command::new("osascript").args(["-e", &script]).output()?;
        if out.status.success() {
            Ok(())
        } else {
            bail!(
                "system proxy change failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use super::*;

    pub fn snapshot() -> Backup {
        Backup::default()
    }
    pub fn enable(_host: &str, _port: u16, _backup: &Backup) -> Result<()> {
        bail!("System proxy control is currently implemented for macOS only");
    }
    pub fn disable(_backup: &Backup) -> Result<()> {
        bail!("System proxy control is currently implemented for macOS only");
    }
}

/// Capture current proxy state for every network service.
pub fn snapshot() -> Backup {
    imp::snapshot()
}

/// Point every service's HTTP/HTTPS proxy at `host:port` (one auth prompt).
pub fn enable(host: &str, port: u16, backup: &Backup) -> Result<()> {
    imp::enable(host, port, backup)
}

/// Restore every service to its snapshotted state (one auth prompt).
pub fn disable(backup: &Backup) -> Result<()> {
    imp::disable(backup)
}
