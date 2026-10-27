//! Exact presence checks in the Windows ROOT stores.
//!
//! The first version asked `certutil -user -store ROOT` and searched its text.
//! That store is the *logical* CurrentUser view, which includes every
//! LocalMachine root — so a CA installed for all users also read as installed
//! for this user, the UI said "this user + all users", and uninstall then ran a
//! pointless per-user delete. Two `certutil` processes per status check did not
//! help either.
//!
//! Here the user side opens the CurrentUser ROOT *registry physical store*
//! only, and the machine side the LocalMachine logical store (which, correctly,
//! includes roots pushed by Group Policy and the enterprise store).
//!
//! Install and uninstall still go through `certutil`: both are user-initiated,
//! both always raise a Windows confirmation dialog whatever API is used, and
//! the machine store needs a UAC-elevated process anyway.

use windows_sys::Win32::Security::Cryptography::{
    CertCloseStore, CertFindCertificateInStore, CertFreeCertificateContext, CertOpenStore,
    CERT_FIND_SHA1_HASH, CERT_STORE_OPEN_EXISTING_FLAG, CERT_STORE_PROV_SYSTEM_REGISTRY_W,
    CERT_STORE_PROV_SYSTEM_W, CERT_STORE_READONLY_FLAG, CERT_SYSTEM_STORE_CURRENT_USER,
    CERT_SYSTEM_STORE_LOCAL_MACHINE, CRYPT_INTEGER_BLOB, PKCS_7_ASN_ENCODING, X509_ASN_ENCODING,
};

use super::wide;

/// Is a certificate with this SHA-1 thumbprint in the current user's own ROOT
/// store (not inherited from the machine)?
pub fn in_user_root(sha1: &[u8]) -> bool {
    find(CERT_STORE_PROV_SYSTEM_REGISTRY_W, CERT_SYSTEM_STORE_CURRENT_USER, sha1)
}

/// Is it trusted machine-wide (LocalMachine ROOT, including policy roots)?
pub fn in_machine_root(sha1: &[u8]) -> bool {
    find(CERT_STORE_PROV_SYSTEM_W, CERT_SYSTEM_STORE_LOCAL_MACHINE, sha1)
}

fn find(provider: windows_sys::core::PCSTR, location: u32, sha1: &[u8]) -> bool {
    if sha1.len() != 20 {
        return false;
    }
    let name = wide("ROOT");
    // SAFETY: `name` is NUL-terminated and outlives the call; read-only open.
    let store = unsafe {
        CertOpenStore(
            provider,
            0,
            0,
            location | CERT_STORE_OPEN_EXISTING_FLAG | CERT_STORE_READONLY_FLAG,
            name.as_ptr().cast(),
        )
    };
    if store.is_null() {
        return false;
    }
    let blob = CRYPT_INTEGER_BLOB { cbData: 20, pbData: sha1.as_ptr() as *mut u8 };
    // SAFETY: `blob` points at 20 live bytes; the found context is freed below.
    let found = unsafe {
        CertFindCertificateInStore(
            store,
            X509_ASN_ENCODING | PKCS_7_ASN_ENCODING,
            0,
            CERT_FIND_SHA1_HASH,
            (&blob as *const CRYPT_INTEGER_BLOB).cast(),
            std::ptr::null(),
        )
    };
    let present = !found.is_null();
    unsafe {
        if present {
            CertFreeCertificateContext(found);
        }
        CertCloseStore(store, 0);
    }
    present
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_thumbprint_nobody_has_is_absent_everywhere() {
        let nobody = [0x5au8; 20];
        assert!(!in_user_root(&nobody));
        assert!(!in_machine_root(&nobody));
    }

    #[test]
    fn a_root_every_windows_machine_trusts_is_found_machine_wide() {
        // Microsoft Root Certificate Authority 2011 — shipped in every
        // supported Windows' LocalMachine ROOT. Proves the store opens and the
        // hash lookup is wired up, without touching any store.
        let ms_2011: [u8; 20] = [
            0x8f, 0x43, 0x28, 0x8a, 0xd2, 0x72, 0xf3, 0x10, 0x3b, 0x6f, 0xb1, 0x42, 0x84, 0x85,
            0xea, 0x30, 0x14, 0xc0, 0xbc, 0xfe,
        ];
        assert!(in_machine_root(&ms_2011));
        assert!(
            !in_user_root(&ms_2011),
            "a machine root must not read as a per-user install — the bug this module fixes"
        );
    }

    #[test]
    fn a_malformed_thumbprint_is_never_found() {
        assert!(!in_machine_root(&[1, 2, 3]));
    }
}
