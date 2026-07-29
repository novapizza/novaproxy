# NovaProxy privileged helper — what it is and how to try it

## Why it exists

Changing the macOS system proxy means running `networksetup`, and `networksetup`
writes machine-wide SystemConfiguration state — so it needs **root**. Without a
helper, NovaProxy buys that privilege the only way an unprivileged app can: by
shelling out to

```
osascript -e 'do shell script "…" with administrator privileges'
```

which is the macOS **password dialog**. Apple's design makes that dialog
password-only (never Touch ID) and provides no way to remember the grant, so it
appeared on *every* proxy toggle.

The worst case was not the toggle. If the app exited uncleanly while the system
proxy was on — a crash, a force-quit, a `cargo tauri dev` reload — it left a
`sysproxy_backup.json` behind, and the next launch tried to restore those
settings during startup. That raised a password dialog **before any window
existed**, which read as "the app asks for my password when it starts".

The helper removes both. You authenticate **once, at install**. After that,
enabling the proxy, disabling it, and recovering from a crash all happen
silently.

Windows and Linux never had this problem — their proxy settings are per-user —
so the helper is macOS-only and the rest of the app is unchanged by it.

## What it actually is

```
  NovaProxy (your user)  ──unix socket──▶  nova-helper (root, launchd)  ──▶  networksetup
```

| Piece | Where |
|---|---|
| Protocol, client, install plan, server loop | [`crates/nova-os/src/helper.rs`](crates/nova-os/src/helper.rs) |
| The daemon binary | [`crates/nova-helper/`](crates/nova-helper/) |
| launchd job | `/Library/LaunchDaemons/dev.novaproxy.helper.plist` |
| Installed binary | `/Library/Application Support/NovaProxy/nova-helper` |
| Socket | `/var/run/novaproxy-helper.sock` |

It speaks a line-delimited JSON protocol with exactly three operations —
`Ping`, `Enable{host,port,backup}`, `Disable{backup}`. There is deliberately **no**
"run this command" endpoint: the daemon can point the system proxy at a loopback
address, or put a snapshot back, and nothing else.

`nova-os` is a separate crate from `nova-core` for exactly this reason. The
daemon runs as root, so it links command plans and system-proxy control and
nothing else — not the MITM engine, its TLS stack, or its JavaScript runtime.
The result is a 2.4 MB root binary instead of a 31 MB one.

## Trying it

### 1. Build the helper binary

```bash
cargo build -p nova-helper
```

In a dev tree the app looks for `nova-helper` next to its own executable
(`target/debug/`), so this is all the wiring needed. You can also point it
somewhere explicit with `NOVAPROXY_HELPER_BIN=/path/to/nova-helper`.

### 2. Install it from the app

```bash
npm run app
```

**Settings ⚙ → General → Privileged helper → Install helper.**

You get one administrator password prompt. That is the last one.

The card tells you which state you are in:

| Card says | Meaning |
|---|---|
| *Not installed — every proxy change asks for your password* | No daemon; the app falls back to the `osascript` prompt |
| *Installed — proxy changes apply silently* | Working |
| *Installed, but speaks protocol N — reinstall to update* | The app was upgraded past the running daemon; **Reinstall** |
| *No helper binary was found next to the app* | Step 1 was skipped |

### 3. Confirm it works

Toggle **System Proxy** in the toolbar a few times. No password dialog should
appear at any point.

From a terminal:

```bash
# the job is loaded
sudo launchctl print system/dev.novaproxy.helper | head -20

# root owns the binary and the plist
ls -l "/Library/Application Support/NovaProxy/nova-helper" \
      /Library/LaunchDaemons/dev.novaproxy.helper.plist

# the socket belongs to you, and only you
ls -l /var/run/novaproxy-helper.sock     # srw------- <your user> wheel

# what it is doing
log stream --predicate 'process == "nova-helper"' --info
```

### 4. Test the crash-recovery path

This is the bug the helper was built for:

1. Turn **System Proxy** on.
2. Kill the app hard (`⌥⌘Esc` → Force Quit, or `kill -9`), leaving
   `~/Library/Application Support/NovaProxy/sysproxy_backup.json` behind.
3. Launch it again.

**With the helper installed:** your settings are restored silently during
startup, and the backup file is gone.

**Without it:** no password dialog — instead a warning bar appears above the flow
list ("Your system proxy still points at NovaProxy from a session that ended
unexpectedly") with a **Restore settings** button. The prompt only happens when
you press it. That is the point: the app never raises the admin dialog on its
own.

### 5. Removing it

**Settings → General → Remove helper** (one prompt), or by hand:

```bash
sudo launchctl bootout system/dev.novaproxy.helper
sudo rm -f /Library/LaunchDaemons/dev.novaproxy.helper.plist \
           "/Library/Application Support/NovaProxy/nova-helper" \
           /var/run/novaproxy-helper.sock
```

The app keeps working after removal — it just goes back to prompting.

## Security notes

The daemon is root, so it treats everything it receives as hostile:

- the socket is owned by the installing uid at mode `0600`, **and** every
  connection re-checks the peer with `getpeereid` — a different local account is
  refused even if the file mode is tampered with;
- proxy hosts must be **loopback literals** (`127.0.0.1`, `::1`), so it cannot be
  talked into routing your traffic through someone else's server;
- ports must be non-zero, and service names must match services the machine
  actually has, so a tampered backup file cannot steer `networksetup`;
- commands are executed as argv, never through a shell, so no value can be read
  as syntax;
- malformed input gets an error reply, not a dead daemon.

**The one accepted gap.** A unix socket cannot verify the *caller's code
signature* the way XPC's `xpc_connection_set_peer_code_signing_requirement` can.
So another process running as **you** could ask the helper to point the system
proxy at a loopback port. That is bounded — loopback proxy settings only, never
CA installs, never arbitrary commands — but it is real. Closing it needs a signed
app: take the peer pid via `LOCAL_PEERPID` and check it against a designated
requirement with `SecCodeCheckValidity`. Until then this is strictly narrower
than a `sudoers` drop-in and strictly wider than XPC-with-signing.

**Why a LaunchDaemon and not `SMAppService`.** `SMAppService`/`SMJobBless`
require a Developer ID signed, notarized bundle with matching code requirements.
A plain LaunchDaemon works from an unsigned local build, which a self-hosted
debugging proxy has to support. Switching to `SMAppService` once the app is
signed is the intended end state and closes the gap above.

## Testing it without root

`NOVAPROXY_HELPER_SOCKET` relocates the socket — **in debug builds only**, so an
environment variable can never move a privileged endpoint in a release build.
That lets the real server loop, the real peer check and the real client run as an
ordinary user:

```bash
cargo test -p nova-os --test helper_ipc
```

Every request in that test is one the helper must **refuse** (malformed JSON,
unknown operations, non-loopback hosts, unknown network services), plus `Ping`.
That is deliberate: a test that got as far as running `networksetup` would
reconfigure the machine running it.

## Packaging caveat

Bundling the helper into the `.app` is **not** wired into `tauri.conf.json`.
Tauri validates `bundle.resources` paths at compile time, so a committed entry
breaks `cargo build` and `cargo test` on a clean checkout where the release
binary does not exist yet.

To package:

```bash
cargo build --release -p nova-helper
```

then add to `src-tauri/tauri.conf.json` under `bundle`:

```json
"resources": { "../target/release/nova-helper": "nova-helper" }
```

and run `cargo tauri build`. `source_binary()` already looks in
`Contents/Resources/`, so the installed app will find it there.
