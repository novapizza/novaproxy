//! Owner-only file permissions — the Windows half of `chmod 0600`.
//!
//! A file in the user profile inherits an ACL that already keeps other
//! *standard* users out, but also grants every administrator and anything
//! running as SYSTEM, and follows whatever a domain administrator set on the
//! profile. For the CA private key — which can mint a certificate for any site
//! this machine trusts — that is too wide. This replaces the inherited ACL
//! with a protected one: the file's owner, and SYSTEM (which backup and the
//! OS itself need; it can read everything regardless).

use anyhow::{bail, Result};
use std::path::Path;
use windows_sys::Win32::Foundation::LocalFree;
use windows_sys::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::{
    SetFileSecurityW, DACL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION,
    PSECURITY_DESCRIPTOR,
};

use super::{wide, wide_path};

/// `P`: protected (no inheritance). `OW`: OWNER RIGHTS, i.e. whoever owns the
/// file. `SY`: LocalSystem. `FA`: full access.
pub const OWNER_ONLY_SDDL: &str = "D:P(A;;FA;;;OW)(A;;FA;;;SY)";

pub fn restrict_to_owner(path: &Path) -> Result<()> {
    let sddl = wide(OWNER_ONLY_SDDL);
    let mut sd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
    // SAFETY: `sddl` is NUL-terminated; `sd` is LocalAlloc'd and freed below.
    let ok = unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut sd,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        bail!("cannot build security descriptor: {}", std::io::Error::last_os_error());
    }
    let file = wide_path(path);
    // SAFETY: `file` is NUL-terminated and `sd` is a valid descriptor.
    let ok = unsafe {
        SetFileSecurityW(
            file.as_ptr(),
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            sd,
        )
    };
    let err = std::io::Error::last_os_error();
    unsafe { LocalFree(sd as _) };
    if ok == 0 {
        bail!("cannot restrict {}: {err}", path.display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_owner_can_still_read_and_rewrite_a_restricted_file() {
        let path = std::env::temp_dir().join(format!("novaproxy-acl-{}.key", std::process::id()));
        std::fs::write(&path, "secret").unwrap();
        restrict_to_owner(&path).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "secret");
        std::fs::write(&path, "rotated").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "rotated");
        std::fs::remove_file(&path).unwrap();
    }
}
