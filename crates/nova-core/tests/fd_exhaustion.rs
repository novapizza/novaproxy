//! Running out of file descriptors is recognised as such. Its own test binary,
//! because it lowers the open-file limit of the whole process.

#![cfg(unix)]

use nova_core::fdlimit;

#[test]
fn a_process_at_its_limit_is_told_apart_from_a_dns_failure() {
    let open = fdlimit::open_count();
    let mut lim = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
    unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut lim) };
    let saved = lim;
    lim.rlim_cur = open + 4;
    assert_eq!(unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &lim) }, 0);

    let mut held = Vec::new();
    while let Ok(f) = std::fs::File::open("/dev/null") {
        held.push(f);
    }
    assert!(fdlimit::exhausted(), "every descriptor is taken");

    held.pop();
    assert!(!fdlimit::exhausted(), "one free descriptor is enough to open one");

    // Raising works from a low limit and stops at what was asked.
    drop(held);
    assert_eq!(fdlimit::raise(open + 64).unwrap(), open + 64);
    assert_eq!(fdlimit::current(), Some(open + 64));

    unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &saved) };
}
