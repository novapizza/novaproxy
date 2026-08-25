//! NovaProxy's privileged helper, run as root by launchd.
//!
//! It exists so the app can change the macOS system proxy without an
//! administrator password every time — including during crash recovery at
//! launch, which used to raise a password dialog before the window appeared.
//!
//! Not meant to be run by hand: launchd starts it from
//! `/Library/LaunchDaemons/dev.novaproxy.helper.plist`, which the app installs
//! from Settings › General. The whole protocol and trust model live in
//! [`nova_os::helper`].

use anyhow::{bail, Result};

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        // stdout is a file now (the plist's StandardOutPath), and ANSI colour
        // escapes in a log a human is meant to read are just noise.
        .with_ansi(false)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "nova_helper=info,nova_os=info".into()),
        )
        .init();

    let owner_uid = owner_uid_from(std::env::args().skip(1))?;

    #[cfg(target_os = "macos")]
    {
        nova_os::helper::server::serve(owner_uid)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = owner_uid;
        bail!("the NovaProxy helper is only needed on macOS")
    }
}

/// Parse `--owner-uid <n>`, the uid the daemon will serve.
///
/// Required rather than defaulted: a helper that guessed would either serve
/// everyone or no one, and both are wrong.
fn owner_uid_from(args: impl Iterator<Item = String>) -> Result<u32> {
    let mut args = args.peekable();
    while let Some(arg) = args.next() {
        if arg == "--owner-uid" {
            let value = args.next().unwrap_or_default();
            return value
                .parse()
                .map_err(|_| anyhow::anyhow!("`{value}` is not a uid"));
        }
    }
    bail!("usage: nova-helper --owner-uid <uid>")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<u32> {
        owner_uid_from(args.iter().map(|s| s.to_string()))
    }

    #[test]
    fn the_owner_uid_is_read_from_the_launchd_arguments() {
        assert_eq!(parse(&["--owner-uid", "501"]).unwrap(), 501);
    }

    #[test]
    fn a_missing_or_bogus_uid_is_refused_rather_than_guessed() {
        assert!(parse(&[]).is_err());
        assert!(parse(&["--owner-uid"]).is_err());
        assert!(parse(&["--owner-uid", "nobody"]).is_err());
    }
}
