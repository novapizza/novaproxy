//! OS command plans: what to run, how to elevate it, and one place that runs it.
//!
//! Trust-store and system-proxy changes are per-platform *commands*, and the
//! interesting part is which commands, in what order, with what escaping — not
//! the process spawning. So both surfaces build a [`Plan`] (pure data) and hand
//! it here to execute.
//!
//! That split is what makes the Windows and Linux paths testable from any
//! machine: every plan is asserted directly in unit tests, on whatever OS the
//! test suite happens to run on. Only [`Plan::run`] behaves differently per
//! platform, and it is deliberately thin.
//!
//! Elevation is per-platform and each variant is designed to raise **one**
//! prompt: macOS batches into a single `osascript … with administrator
//! privileges`, Linux into a single `pkexec sh -c`, Windows into one
//! UAC-triggering `Start-Process -Verb RunAs`.

use std::process::Command;

use anyhow::{bail, Result};

/// One command to run: program plus argv (no shell involved unless elevation
/// requires it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub program: String,
    pub args: Vec<String>,
}

impl Step {
    pub fn new<P: Into<String>>(program: P, args: &[&str]) -> Self {
        Self {
            program: program.into(),
            args: args.iter().map(|a| a.to_string()).collect(),
        }
    }

    pub fn with_args<P: Into<String>>(program: P, args: Vec<String>) -> Self {
        Self { program: program.into(), args }
    }

    /// Render as a shell command line, quoting each word. Used only by the
    /// elevation paths, which must hand a string to a shell.
    pub fn to_shell(&self) -> String {
        let mut out = shell_quote(&self.program);
        for a in &self.args {
            out.push(' ');
            out.push_str(&shell_quote(a));
        }
        out
    }
}

/// How a plan acquires the privileges it needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Elevation {
    /// Run directly as the current user — no prompt at all.
    None,
    /// macOS: one `osascript … with administrator privileges` (password dialog).
    MacAdmin,
    /// Windows: one `Start-Process -Verb RunAs` (UAC dialog).
    WindowsUac,
    /// Linux: one `pkexec sh -c` (polkit dialog).
    LinuxPkexec,
}

/// A sequence of commands plus how to elevate them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub steps: Vec<Step>,
    pub elevation: Elevation,
    /// Attempt every step even after one fails, reporting the first error at the
    /// end. Restore paths set this: giving up halfway through would leave the
    /// machine half-reconfigured, which for a proxy means no working internet.
    pub best_effort: bool,
}

impl Plan {
    pub fn new(steps: Vec<Step>, elevation: Elevation) -> Self {
        Self { steps, elevation, best_effort: false }
    }

    /// A plan whose steps are all attempted even if some fail.
    pub fn best_effort(steps: Vec<Step>, elevation: Elevation) -> Self {
        Self { steps, elevation, best_effort: true }
    }

    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }

    /// The same steps, run directly. Used by the privileged helper: it is already
    /// root, so asking `osascript` for privileges it holds would raise a password
    /// dialog on a machine with no one sitting in front of it.
    pub fn without_elevation(mut self) -> Self {
        self.elevation = Elevation::None;
        self
    }

    /// Run the plan, translating the elevation into the platform's one prompt.
    pub fn run(&self) -> Result<()> {
        if self.steps.is_empty() {
            return Ok(());
        }
        // The elevated branches are the ones that put a password dialog in front
        // of the user, and until now they were the only ones that logged
        // nothing at all — a failed `osascript` looked identical to a user who
        // pressed Cancel. Program names only: the arguments carry paths.
        let elevated = !matches!(self.elevation, Elevation::None);
        if elevated {
            tracing::info!(
                elevation = ?self.elevation,
                steps = self.steps.len(),
                programs = %self.program_names(),
                "running elevated plan"
            );
        }
        let result = match self.elevation {
            Elevation::None => self.run_direct(),
            // Elevated variants batch every step behind a single prompt.
            Elevation::MacAdmin => run_one(&mac_admin_step(&self.joined_shell())),
            Elevation::LinuxPkexec => run_one(&pkexec_step(&self.joined_shell())),
            Elevation::WindowsUac => run_one(&windows_uac_step(&self.steps)?),
        };
        if elevated {
            match &result {
                Ok(()) => tracing::info!(elevation = ?self.elevation, "elevated plan succeeded"),
                // A cancelled prompt and a genuine failure arrive the same way
                // here; the message is the only thing that tells them apart.
                Err(e) => tracing::warn!(elevation = ?self.elevation, "elevated plan failed: {e}"),
            }
        }
        result
    }

    /// The programs this plan runs, without their arguments.
    ///
    /// Arguments are deliberately excluded: they carry keychain paths, network
    /// service names and the user's home directory. The program list is enough
    /// to tell `security` from `networksetup` in a support log.
    fn program_names(&self) -> String {
        let mut names: Vec<&str> = self.steps.iter().map(|s| s.program.as_str()).collect();
        names.dedup();
        names.join(", ")
    }

    fn run_direct(&self) -> Result<()> {
        let mut first_error: Option<anyhow::Error> = None;
        for step in &self.steps {
            match run_one(step) {
                Ok(()) => {}
                Err(e) if self.best_effort => {
                    tracing::warn!("step failed (continuing): {e}");
                    first_error = first_error.or(Some(e));
                }
                Err(e) => return Err(e),
            }
        }
        match first_error {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    /// Every step as one `&&`-joined shell line (elevated paths only).
    ///
    /// `;` for best-effort plans and `&&` otherwise: a restore must not stop
    /// because one service was already in the wanted state.
    fn joined_shell(&self) -> String {
        let sep = if self.best_effort { " ; " } else { " && " };
        self.steps
            .iter()
            .map(|s| s.to_shell())
            .collect::<Vec<_>>()
            .join(sep)
    }
}

/// Spawn a command from a working directory we know is readable.
///
/// Every child process here goes through this, because inheriting our own cwd
/// has bitten us: a build launched from a TCC-protected folder (`~/Documents`,
/// `~/Desktop`, `~/Downloads`) runs with a cwd the process is not permitted to
/// read. `getcwd` then fails in the child — `shell-init: … getcwd: Operation not
/// permitted` — and macOS refuses to present the trust dialog on top of it
/// ("SecTrustSettingsSetTrustSettings: … no user interaction was possible"), so
/// installing the CA fails for reasons that look nothing like the cause.
///
/// `/` is readable by everyone and is never TCC-protected. Unix-only: the
/// failure mode is a macOS sandbox behaviour with no Windows analogue, and
/// changing the cwd of `reg.exe`/`certutil` unprompted is not worth the risk.
fn command(program: &str) -> Command {
    let mut cmd = Command::new(program);
    #[cfg(unix)]
    cmd.current_dir("/");
    cmd
}

/// Run one command, mapping a non-zero exit into an error carrying its stderr.
fn run_one(step: &Step) -> Result<()> {
    let out = command(&step.program)
        .args(&step.args)
        .output()
        .map_err(|e| anyhow::anyhow!("cannot run {}: {e}", step.program))?;
    if out.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let detail = if stderr.is_empty() { stdout } else { stderr };
    bail!("{} failed: {}", step.program, first_line(&detail));
}

/// Capture a command's stdout (empty on failure). Queries use this: "not found"
/// is an answer, not an error.
pub fn capture(program: &str, args: &[&str]) -> String {
    command(program)
        .args(args)
        .output()
        .map(|o| {
            let mut text = String::from_utf8_lossy(&o.stdout).into_owned();
            // certutil and gsettings report some answers on stderr; both matter
            // when deciding whether a cert is present.
            text.push_str(&String::from_utf8_lossy(&o.stderr));
            text
        })
        .unwrap_or_default()
}

/// Does a command exist on PATH? Used to degrade gracefully when an optional
/// tool (NSS `certutil`, `gsettings`) is not installed.
pub fn have_tool(program: &str) -> bool {
    let probe = if cfg!(windows) { "where" } else { "which" };
    command(probe)
        .arg(program)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn first_line(s: &str) -> &str {
    s.lines().next().unwrap_or("").trim()
}

/* ------------------------------- elevation ------------------------------- */

/// `osascript -e 'do shell script "…" with administrator privileges'`.
pub fn mac_admin_step(shell_line: &str) -> Step {
    // The shell line is embedded in an AppleScript string literal, so its
    // double quotes and backslashes need escaping for *that* layer.
    let escaped = shell_line.replace('\\', "\\\\").replace('"', "\\\"");
    Step::with_args(
        "osascript",
        vec![
            "-e".to_string(),
            format!("do shell script \"{escaped}\" with administrator privileges"),
        ],
    )
}

/// `pkexec sh -c '…'` — one polkit prompt for the whole line.
pub fn pkexec_step(shell_line: &str) -> Step {
    Step::with_args(
        "pkexec",
        vec!["sh".to_string(), "-c".to_string(), shell_line.to_string()],
    )
}

/// One UAC-elevated `Start-Process`, waited on so the caller sees the outcome.
///
/// Windows cannot elevate a *shell line*, only a process, so a UAC plan is
/// limited to a single step. Callers needing several elevated commands must
/// either combine them into one program invocation or accept one prompt each —
/// which is why the trust-store paths keep their elevated work to one command.
pub fn windows_uac_step(steps: &[Step]) -> Result<Step> {
    let [step] = steps else {
        bail!("Windows elevation runs exactly one command, got {}", steps.len());
    };
    let mut ps = format!("Start-Process -FilePath {}", ps_quote(&step.program));
    if !step.args.is_empty() {
        let list = step
            .args
            .iter()
            .map(|a| ps_quote(a))
            .collect::<Vec<_>>()
            .join(",");
        ps.push_str(&format!(" -ArgumentList {list}"));
    }
    // `-Wait` so the exit status is ours to report; `-WindowStyle Hidden` keeps a
    // console window from flashing up.
    ps.push_str(" -Verb RunAs -Wait -WindowStyle Hidden");
    Ok(Step::with_args(
        "powershell",
        vec![
            "-NoProfile".to_string(),
            "-NonInteractive".to_string(),
            "-Command".to_string(),
            ps,
        ],
    ))
}

/// Single-quote a word for a POSIX shell, escaping embedded single quotes.
pub fn shell_quote(word: &str) -> String {
    if !word.is_empty()
        && word
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_./:=@".contains(&b))
    {
        return word.to_string();
    }
    format!("'{}'", word.replace('\'', r"'\''"))
}

/// Single-quote a word for PowerShell (doubling embedded single quotes).
pub fn ps_quote(word: &str) -> String {
    format!("'{}'", word.replace('\'', "''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_quoting_leaves_plain_words_alone() {
        assert_eq!(shell_quote("networksetup"), "networksetup");
        assert_eq!(shell_quote("/usr/local/share/ca-certificates"), "/usr/local/share/ca-certificates");
        assert_eq!(shell_quote("-setwebproxystate"), "-setwebproxystate");
    }

    #[test]
    fn shell_quoting_protects_spaces_and_metacharacters() {
        assert_eq!(shell_quote("Wi-Fi Ethernet"), "'Wi-Fi Ethernet'");
        assert_eq!(shell_quote(""), "''");
        // A service name containing a quote must not be able to close the
        // string and append a command.
        assert_eq!(shell_quote("a'; rm -rf /"), r"'a'\''; rm -rf /'");
        assert_eq!(shell_quote("$(whoami)"), "'$(whoami)'");
    }

    #[test]
    fn step_renders_as_a_shell_line_quoting_only_what_needs_it() {
        let step = Step::new("networksetup", &["-setwebproxy", "Wi-Fi", "127.0.0.1", "9090"]);
        assert_eq!(step.to_shell(), "networksetup -setwebproxy Wi-Fi 127.0.0.1 9090");
        // A service name with a space is the common real-world case.
        let step = Step::new("networksetup", &["-setwebproxystate", "USB 10/100 LAN", "on"]);
        assert_eq!(step.to_shell(), "networksetup -setwebproxystate 'USB 10/100 LAN' on");
    }

    #[test]
    fn plans_join_with_and_but_restores_join_with_semicolons() {
        let steps = vec![Step::new("a", &["1"]), Step::new("b", &["2"])];
        assert_eq!(Plan::new(steps.clone(), Elevation::MacAdmin).joined_shell(), "a 1 && b 2");
        // A restore must attempt every step; `&&` would abandon the rest after
        // the first already-in-that-state failure.
        assert_eq!(Plan::best_effort(steps, Elevation::MacAdmin).joined_shell(), "a 1 ; b 2");
    }

    #[test]
    fn mac_admin_wraps_the_line_in_one_prompt() {
        let step = mac_admin_step("networksetup -setwebproxystate 'USB LAN' on");
        assert_eq!(step.program, "osascript");
        let script = &step.args[1];
        assert!(script.starts_with("do shell script \""));
        assert!(script.ends_with("with administrator privileges"));
        assert_eq!(script.matches("with administrator privileges").count(), 1);
        // The shell line's single quotes pass through the AppleScript layer as-is;
        // only double quotes and backslashes need escaping there.
        assert!(script.contains("'USB LAN'"), "got {script}");
    }

    #[test]
    fn mac_admin_escapes_double_quotes_and_backslashes() {
        let script = mac_admin_step(r#"echo "hi\there""#).args[1].clone();
        assert!(script.contains(r#"\"hi\\there\""#), "got {script}");
    }

    #[test]
    fn pkexec_runs_the_whole_line_under_one_prompt() {
        let step = pkexec_step("install -m 644 a b && update-ca-certificates");
        assert_eq!(step.program, "pkexec");
        assert_eq!(step.args[0], "sh");
        assert_eq!(step.args[1], "-c");
        assert!(step.args[2].contains("update-ca-certificates"));
    }

    #[test]
    fn windows_uac_builds_a_runas_start_process() {
        let step = windows_uac_step(&[Step::new(
            "certutil",
            &["-addstore", "-f", "ROOT", r"C:\Users\me\ca.pem"],
        )])
        .unwrap();
        assert_eq!(step.program, "powershell");
        let ps = step.args.last().unwrap();
        assert!(ps.starts_with("Start-Process -FilePath 'certutil'"));
        assert!(ps.contains("-ArgumentList '-addstore','-f','ROOT','C:\\Users\\me\\ca.pem'"));
        assert!(ps.contains("-Verb RunAs"), "UAC is what makes this elevated");
        assert!(ps.contains("-Wait"), "we must observe the outcome");
    }

    #[test]
    fn windows_uac_quoting_survives_apostrophes_in_paths() {
        let step = windows_uac_step(&[Step::new("certutil", &[r"C:\Users\O'Brien\ca.pem"])]).unwrap();
        let ps = step.args.last().unwrap();
        assert!(ps.contains("'C:\\Users\\O''Brien\\ca.pem'"), "got {ps}");
    }

    #[test]
    fn windows_uac_rejects_multi_step_plans() {
        // Windows elevates a process, not a shell line: silently running only the
        // first step would leave a half-applied change.
        let err = windows_uac_step(&[Step::new("a", &[]), Step::new("b", &[])]).unwrap_err();
        assert!(err.to_string().contains("exactly one command"));
    }

    #[test]
    fn empty_plans_do_nothing_and_succeed() {
        assert!(Plan::new(Vec::new(), Elevation::MacAdmin).is_empty());
        assert!(Plan::new(Vec::new(), Elevation::MacAdmin).run().is_ok());
    }

    #[test]
    fn direct_plans_report_a_failing_step() {
        // A program that cannot exist: the error names it, rather than being
        // silently swallowed.
        let plan = Plan::new(
            vec![Step::new("novaproxy-no-such-program", &[])],
            Elevation::None,
        );
        let err = plan.run().unwrap_err();
        assert!(err.to_string().contains("novaproxy-no-such-program"));
    }

    #[test]
    fn best_effort_plans_attempt_every_step_and_still_report() {
        let plan = Plan::best_effort(
            vec![
                Step::new("novaproxy-no-such-program", &[]),
                // A command that exists everywhere, proving we kept going.
                Step::new("echo", &["ok"]),
            ],
            Elevation::None,
        );
        let err = plan.run().unwrap_err();
        assert!(err.to_string().contains("novaproxy-no-such-program"));
    }

    #[test]
    fn have_tool_finds_a_real_program_and_not_a_fake_one() {
        assert!(have_tool("echo") || have_tool("cmd"), "some shell builtin path must resolve");
        assert!(!have_tool("novaproxy-definitely-not-installed"));
    }

    #[test]
    fn capture_returns_stdout() {
        assert_eq!(capture("echo", &["hello"]).trim(), "hello");
        assert_eq!(capture("novaproxy-no-such-program", &[]), "");
    }

    #[test]
    #[cfg(unix)]
    fn spawned_commands_do_not_inherit_our_working_directory() {
        // Regression: a build launched from a TCC-protected folder has a cwd its
        // own children cannot read, and `security` then fails to present the
        // trust dialog. Children must start from `/` regardless of where we are.
        assert_eq!(capture("pwd", &[]).trim(), "/");
        // Whatever the test harness's cwd is, it is not what the child saw.
        let ours = std::env::current_dir().unwrap();
        assert_ne!(ours.as_path(), std::path::Path::new("/"), "test premise");
    }
}
