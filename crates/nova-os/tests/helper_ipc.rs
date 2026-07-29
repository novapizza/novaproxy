//! End-to-end test of the privileged helper's socket protocol.
//!
//! The daemon normally runs as root on `/var/run/novaproxy-helper.sock`, which no
//! test can reach. `NOVAPROXY_HELPER_SOCKET` (honoured in debug builds only)
//! relocates it, so the real server loop, the real peer check and the real client
//! can be exercised as an ordinary user.
//!
//! Every request here is one the helper must **refuse**, plus `Ping`. That is
//! deliberate: a test that got as far as running `networksetup` would reconfigure
//! the machine it is running on.

#![cfg(target_os = "macos")]

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use nova_os::helper::{self, Response};
use nova_os::sysproxy::{Backup, ServiceBackup};

fn socket_for_this_test() -> PathBuf {
    std::env::temp_dir().join(format!("nova-helper-{}.sock", std::process::id()))
}

/// Speak the protocol by hand, so malformed input can be sent too.
fn raw(line: &str) -> Response {
    let stream = UnixStream::connect(helper::socket_path()).expect("connect");
    (&stream).write_all(line.as_bytes()).expect("write");
    (&stream).write_all(b"\n").expect("write");
    let mut reply = String::new();
    BufReader::new(&stream).read_line(&mut reply).expect("read");
    serde_json::from_str(&reply).expect("a JSON reply")
}

fn service(name: &str) -> ServiceBackup {
    ServiceBackup {
        service: name.to_string(),
        web_enabled: true,
        web_host: "10.0.0.1".into(),
        web_port: "8080".into(),
        secure_enabled: false,
        secure_host: String::new(),
        secure_port: String::new(),
    }
}

/// One test rather than several: they share one process-wide environment
/// variable and one daemon, and running them in parallel would race on both.
#[test]
fn the_helper_answers_pings_and_refuses_everything_dangerous() {
    let path = socket_for_this_test();
    let _ = std::fs::remove_file(&path);
    std::env::set_var("NOVAPROXY_HELPER_SOCKET", &path);

    let uid = helper::current_uid();
    std::thread::spawn(move || {
        let _ = helper::server::serve(uid);
    });

    // launchd would wait on the socket appearing; here we do it ourselves.
    let deadline = Instant::now() + Duration::from_secs(5);
    while !path.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(path.exists(), "the daemon never bound {}", path.display());

    // The handshake the app uses to decide whether a helper is installed.
    assert_eq!(helper::ping().unwrap(), helper::PROTOCOL_VERSION);
    assert!(helper::usable());

    // Garbage must come back as an error, not as a dead daemon: the next request
    // still has to work.
    let reply = raw("{ this is not json");
    assert!(!reply.ok);
    assert!(reply.error.unwrap().contains("malformed"));
    assert_eq!(helper::ping().unwrap(), helper::PROTOCOL_VERSION, "still serving");

    // An unknown operation is likewise a reply, not a crash.
    assert!(!raw(r#"{"op":"reboot"}"#).ok);

    // The attack the loopback rule exists for: routing the machine's traffic
    // through someone else's server. Refused before any command runs.
    let backup = Backup { services: vec![service("Wi-Fi")], ..Default::default() };
    let err = helper::enable("10.0.0.1", 9090, &backup).unwrap_err().to_string();
    assert!(err.contains("loopback"), "got {err}");

    // A snapshot naming a service this machine does not have is refused too, so
    // a tampered backup file cannot steer `networksetup`.
    let bogus = Backup {
        services: vec![service("Not A Real Network Service")],
        ..Default::default()
    };
    let err = helper::disable(&bogus).unwrap_err().to_string();
    assert!(err.contains("not a network service"), "got {err}");

    // And the daemon survived all of it.
    assert!(helper::usable());

    let _ = std::fs::remove_file(&path);
    std::env::remove_var("NOVAPROXY_HELPER_SOCKET");
}
