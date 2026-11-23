//! Systemd job scopes for agent-launched detached workloads (58.1 D-11/D-12).
//!
//! A terminal command that detaches a process (`nohup`, `setsid`, `disown`, a
//! lone background `&`) would otherwise stay in the gateway unit's cgroup:
//! its memory is charged to the gateway, and a gateway restart kills it. On
//! hosts with a user systemd manager, [`crate::backend::local::LocalEnvironment`]
//! instead launches such a command as
//! `systemd-run --user --scope --slice=ironhermes-jobs.slice --unit=ironhermes-job-<id> -- bash -c <script>`
//! so the workload gets its own cgroup outside the gateway unit. Everything
//! else (macOS, Docker, no user bus, `systemd-run` missing) degrades to plain
//! `bash -c`, exactly the pre-58.1 behaviour.
//!
//! Kill switch: set `IRONHERMES_JOB_SCOPE=off` (e.g. in `~/.ironhermes/.env`,
//! which both systemd units load via `EnvironmentFile=`) to skip scoping and
//! the launcher probe entirely.
//!
//! This is placement, not a cap: no per-job memory property is set (a render
//! legitimately needs 60 GB+), and the scoped path is never tied to the
//! gateway's lifetime.

use std::collections::HashMap;
use std::sync::LazyLock;
use std::time::Duration;

use regex::Regex;
use tokio::sync::Mutex;
use tokio::time::Instant;

/// Overrides the launcher binary (test seam / non-standard install). Read from
/// the host process env only; deliberately not in `SAFE_ENV_KEYS`, so it never
/// reaches a child.
pub const SYSTEMD_RUN_ENV: &str = "IRONHERMES_SYSTEMD_RUN";
/// Operator switch; see the module docs. Host-process env only, like
/// [`SYSTEMD_RUN_ENV`].
pub const JOB_SCOPE_ENV: &str = "IRONHERMES_JOB_SCOPE";
/// Slice every job scope lives in; an operator may cap it in aggregate.
pub const JOB_SLICE: &str = "ironhermes-jobs.slice";
/// Prefix of every transient unit name.
pub const JOB_UNIT_PREFIX: &str = "ironhermes-job-";

const DEFAULT_LAUNCHER: &str = "systemd-run";
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);
/// A failed probe is retried after this long, so one transient timeout (a host
/// under memory pressure) cannot disable containment until restart.
const REPROBE_AFTER: Duration = Duration::from_secs(60);

// shortcut: textual heuristic, not a shell parser. A miss equals pre-58.1 behaviour. A false positive (`grep -r nohup .`) costs a ~50 ms scope AND, if that command later times out, its grandchildren outlive the gateway in the scope (see `scope_hint`); tighten to command-position tokens if a real false positive shows up. Extend from blackbox evidence (tmux/screen) if a real miss shows up.
// - the keywords must be bare words (`nohup.out`, `nohup-x` do not match);
// - a lone `&` ends a word at whitespace, `;`, a closer (`)` `}`) or a quote, so `(srv &)` and `bash -c 'srv &'` match.
static DETACH_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?m)\b(nohup|setsid|disown)([^.\w-]|$)|(^|[^&>|<])&([\s;)}'"]|$)"#)
        .expect("static regex")
});

/// True when the RAW command text detaches a process. Must be fed the user's
/// command, never the `eval '<quoted>'` wrapper.
pub fn is_detached_launch(cmd: &str) -> bool {
    DETACH_RE.is_match(cmd)
}

/// The launcher argv for one scoped job; every element is a separate argv
/// entry (no shell interpolation of `script`).
pub fn scope_argv(unit_suffix: &str, script: &str) -> Vec<String> {
    vec![
        "--user".into(),
        "--scope".into(),
        "--quiet".into(),
        "--collect".into(),
        format!("--slice={JOB_SLICE}"),
        format!("--unit={JOB_UNIT_PREFIX}{unit_suffix}"),
        "--description=IronHermes agent job".into(),
        "--".into(),
        "bash".into(),
        "-c".into(),
        script.into(),
    ]
}

/// Header prepended to a scoped launch's output so the model can find and stop
/// its job (head, not tail: output truncation keeps the head).
pub fn scope_hint(unit_suffix: &str) -> String {
    format!(
        "[ironhermes] detached into {JOB_UNIT_PREFIX}{unit_suffix}.scope (own cgroup, survives gateway restarts); list: systemctl --user list-units '{JOB_UNIT_PREFIX}*'; stop: systemctl --user stop {JOB_UNIT_PREFIX}{unit_suffix}.scope\n"
    )
}

/// Extra note for a scoped command that was killed by its timeout (124) or by
/// cancellation (130): only the foreground shell is killed, so anything it
/// already started stays alive in the scope until stopped.
pub fn scope_killed_note(unit_suffix: &str) -> String {
    format!(
        "[ironhermes] the foreground shell was killed, but processes it started stay running in {JOB_UNIT_PREFIX}{unit_suffix}.scope until you stop it\n"
    )
}

/// Runs the launcher once with the exact env the real launch will see
/// (`env_clear()` then `safe_env`), so a missing `XDG_RUNTIME_DIR` or user bus
/// is caught here rather than after the user command was handed over.
async fn probe(launcher: &str, safe_env: &HashMap<String, String>) -> bool {
    let mut cmd = tokio::process::Command::new(launcher);
    cmd.args(["--user", "--scope", "--quiet", "--collect", "--", "true"])
        .env_clear()
        .envs(safe_env)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    matches!(
        tokio::time::timeout(PROBE_TIMEOUT, cmd.status()).await,
        Ok(Ok(status)) if status.success()
    )
}

/// Probe outcome for the default launcher. Only success is permanent.
enum ProbeState {
    Unknown,
    Ok,
    FailedAt(Instant),
}

static DEFAULT_PROBE: Mutex<ProbeState> = Mutex::const_new(ProbeState::Unknown);

/// Resolves availability of `launcher` against `state`: success is cached for
/// good, a failure is cached for `reprobe_after` and then re-probed. Only the
/// first failure is logged at `warn!`, so macOS/containers stay quiet after it.
async fn launcher_available(
    state: &Mutex<ProbeState>,
    launcher: &str,
    safe_env: &HashMap<String, String>,
    reprobe_after: Duration,
) -> bool {
    let mut st = state.lock().await;
    let first_attempt = match *st {
        ProbeState::Ok => return true,
        ProbeState::FailedAt(at) if at.elapsed() < reprobe_after => return false,
        ProbeState::FailedAt(_) => false,
        ProbeState::Unknown => true,
    };
    if probe(launcher, safe_env).await {
        *st = ProbeState::Ok;
        return true;
    }
    *st = ProbeState::FailedAt(Instant::now());
    if first_attempt {
        tracing::warn!(
            target: "ironhermes_exec::scope",
            "systemd-run --user --scope unavailable; detached launches run in the caller's cgroup (re-probing every {}s)",
            reprobe_after.as_secs()
        );
    }
    false
}

/// The launcher to use for a detached command, or `None` to run plain bash.
///
/// With [`SYSTEMD_RUN_ENV`] set the override is probed on every call
/// (uncached, so integration tests stay independent). Otherwise `systemd-run`
/// is probed until it succeeds, then cached; a failure is re-probed after
/// [`REPROBE_AFTER`].
pub async fn scope_launcher(safe_env: &HashMap<String, String>) -> Option<String> {
    // Operator kill switch: checked before any probe.
    if std::env::var(JOB_SCOPE_ENV).is_ok_and(|v| v.eq_ignore_ascii_case("off")) {
        return None;
    }
    if let Ok(over) = std::env::var(SYSTEMD_RUN_ENV) {
        return probe(&over, safe_env).await.then_some(over);
    }
    let ok = launcher_available(&DEFAULT_PROBE, DEFAULT_LAUNCHER, safe_env, REPROBE_AFTER).await;
    ok.then(|| DEFAULT_LAUNCHER.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detached_launch_truth_table() {
        let yes = [
            "cd ~/ComfyUI && CPATH=/x setsid nohup ~/comfyui-env/bin/python main.py --port 8188 > ~/comfyui.log 2>&1 < /dev/null & disown; sleep 30; curl -s localhost:8188",
            "nohup python render.py &",
            "python server.py &",
            "python server.py & sleep 5; curl localhost:8000",
            "setsid ./run.sh",
            "./job.sh >/dev/null 2>&1 &",
            "sleep 4242 & disown",
            "python a.py &\necho started",
            "(python server.py &)",
            "bash -c 'python server.py &'",
            "{ python server.py & }",
        ];
        for c in yes {
            assert!(is_detached_launch(c), "should detect: {c:?}");
        }
        let no = [
            "echo hi",
            "make && make install",
            "cargo build 2>&1 | tail -5",
            "foo |& tee log",
            "ls &>/dev/null",
            "curl 'http://x/?a=1&b=2'",
            "echo a >&2",
            "true || false",
            "tail -f nohup.out",
            "cat setsid.log",
        ];
        for c in no {
            assert!(!is_detached_launch(c), "should NOT detect: {c:?}");
        }
    }

    /// Fake launcher: counts invocations in `count`, succeeds only once `ok` exists.
    #[cfg(unix)]
    fn flaky_launcher(dir: &std::path::Path) -> (String, std::path::PathBuf, std::path::PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let count = dir.join("count");
        let ok = dir.join("ok");
        let script = dir.join("flaky");
        std::fs::write(
            &script,
            format!(
                "#!/usr/bin/env bash\necho x >> '{}'\n[ -e '{}' ]\n",
                count.display(),
                ok.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        (script.to_string_lossy().into_owned(), count, ok)
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn failed_probe_is_retried_and_success_is_cached() {
        let dir = tempfile::TempDir::new().unwrap();
        let (launcher, count, ok) = flaky_launcher(dir.path());
        let env = HashMap::new();
        let state = Mutex::new(ProbeState::Unknown);
        let probes = || {
            std::fs::read_to_string(&count)
                .unwrap_or_default()
                .lines()
                .count()
        };

        // Fails; a long re-probe window means the next call does not re-probe.
        assert!(!launcher_available(&state, &launcher, &env, Duration::from_secs(3600)).await);
        assert!(!launcher_available(&state, &launcher, &env, Duration::from_secs(3600)).await);
        assert_eq!(probes(), 1, "failure must be cached within the window");

        // Window elapsed (zero) and the launcher recovered: re-probed, now cached for good.
        std::fs::write(&ok, "").unwrap();
        assert!(launcher_available(&state, &launcher, &env, Duration::ZERO).await);
        std::fs::remove_file(&ok).unwrap();
        assert!(launcher_available(&state, &launcher, &env, Duration::ZERO).await);
        assert_eq!(probes(), 2, "success must be cached");
    }

    #[test]
    fn scope_argv_exact() {
        assert_eq!(
            scope_argv("ab12cd34", "echo x"),
            vec![
                "--user",
                "--scope",
                "--quiet",
                "--collect",
                "--slice=ironhermes-jobs.slice",
                "--unit=ironhermes-job-ab12cd34",
                "--description=IronHermes agent job",
                "--",
                "bash",
                "-c",
                "echo x",
            ]
        );
    }

    #[test]
    fn scope_hint_names_unit_and_commands() {
        let h = scope_hint("ab12cd34");
        assert!(h.contains("ironhermes-job-ab12cd34.scope"), "{h}");
        assert!(
            h.contains("systemctl --user list-units 'ironhermes-job-*'"),
            "{h}"
        );
        assert!(
            h.contains("systemctl --user stop ironhermes-job-ab12cd34.scope"),
            "{h}"
        );
    }
}
