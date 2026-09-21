//! Phase 52 Plan 01 (Wave 0, D-03b): a test-only stub script whose reply
//! CHANGES between calls.
//!
//! Every `write_stub_script` copy already in this crate (`cli_handoff.rs:1511`,
//! `group_chat_api.rs:1058`, `mention_handoff_api.rs:337`) writes a single
//! fixed output — none of them can express "answer differently on the second
//! invocation," which is exactly what D-03b's leader-retries-exactly-once
//! test needs (RESEARCH Wave 0 Gaps: "no precedent exists, every
//! `write_stub_script` copy writes a single fixed output").
//!
//! This is a module, not a fourth private duplicate, because its consumer —
//! Plan 04's `group_team_api.rs` `mod tests` — cannot reach into another
//! module's private `mod tests` (Round 1 codex MEDIUM, re-confirmed this
//! session: `cli_handoff.rs:1455` is a bare `mod tests`, and its
//! `write_stub_script` at `:1511` is unreachable from a sibling module).
//! `profile_fixture.rs` is this crate's existing precedent for a test
//! fixture shared across sibling `server` modules — same `pub(crate) mod`
//! shape, same `#![cfg(all(test, feature = "server"))]` gate as the test
//! modules that consume it, declared beside it in `server/mod.rs`.
#![cfg(all(test, feature = "server"))]

use std::path::{Path, PathBuf};

/// Writes a `/bin/sh` script at `dir/name` that returns a DIFFERENT output
/// on each invocation, clamping to the last element once the invocation
/// count exceeds `outputs.len()`. Tracks its own invocation count in a
/// sibling counter file `{name}.count` in the same `dir`, readable via
/// [`counting_stub_invocations`] — a test asserting "exactly two calls, never
/// three" needs the counter, not just stdout. Mirrors `write_stub_script`'s
/// 0o755 permission handling verbatim.
///
/// Each output is embedded via a `case` over the counter value (no runtime
/// array indexing in the shell script) and shell-quoted with `'...'`
/// (embedded `'` escaped as `'\''`), so a fenced JSON payload containing
/// backticks, braces, and newlines survives verbatim through the script.
pub(crate) fn write_counting_stub_script(dir: &Path, name: &str, outputs: &[&str]) -> PathBuf {
    assert!(
        !outputs.is_empty(),
        "write_counting_stub_script requires at least one output"
    );

    let path = dir.join(name);
    let counter_path = dir.join(format!("{name}.count"));

    let last_index = outputs.len() - 1;
    let mut case_arms = String::new();
    for (i, output) in outputs.iter().enumerate() {
        let quoted = shell_quote_single(output);
        let pattern = if i == last_index {
            // Clamp: the last arm is also the catch-all, so any count at or
            // beyond the last index reuses the final output.
            "*".to_string()
        } else {
            i.to_string()
        };
        case_arms.push_str(&format!("  {pattern}) printf '%s' {quoted} ;;\n"));
    }

    let counter_quoted =
        shell_quote_single(counter_path.to_str().expect("counter path must be utf8"));
    let script = [
        "#!/bin/sh".to_string(),
        format!("count_file={counter_quoted}"),
        "if [ -f \"$count_file\" ]; then".to_string(),
        "  n=$(cat \"$count_file\")".to_string(),
        "else".to_string(),
        "  n=0".to_string(),
        "fi".to_string(),
        "case \"$n\" in".to_string(),
        case_arms,
        "esac".to_string(),
        "n=$((n + 1))".to_string(),
        "echo \"$n\" > \"$count_file\"".to_string(),
        String::new(),
    ]
    .join("\n");

    std::fs::write(&path, script).expect("write counting stub script");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&path).expect("metadata").permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).expect("set_permissions 0755");
    }
    path
}

/// Reads `{name}.count` in `dir`, returning 0 when absent (no invocation
/// yet). Lets a test assert an exact invocation count rather than inferring
/// one from stdout alone.
pub(crate) fn counting_stub_invocations(dir: &Path, name: &str) -> u32 {
    let counter_path = dir.join(format!("{name}.count"));
    match std::fs::read_to_string(&counter_path) {
        Ok(contents) => contents.trim().parse().unwrap_or(0),
        Err(_) => 0,
    }
}

/// Shell-quotes `value` in single quotes, escaping any embedded `'` as
/// `'\''` — the standard POSIX-shell single-quote escape, robust to
/// backticks, `$`, braces, and newlines (everything a fenced JSON contract
/// payload might contain).
fn shell_quote_single(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counting_stub_script_returns_a_different_reply_on_its_second_invocation() {
        let dir = tempfile::tempdir().expect("tempdir");
        let script = write_counting_stub_script(
            dir.path(),
            "counting-stub.sh",
            &["first reply", "second reply"],
        );

        let run = || {
            std::process::Command::new(&script)
                .output()
                .expect("run counting stub script")
        };

        let first = run();
        assert_eq!(
            String::from_utf8_lossy(&first.stdout),
            "first reply",
            "call 1 stdout must equal the first output"
        );

        let second = run();
        assert_eq!(
            String::from_utf8_lossy(&second.stdout),
            "second reply",
            "call 2 stdout must equal the second output"
        );

        let third = run();
        assert_eq!(
            String::from_utf8_lossy(&third.stdout),
            "second reply",
            "call 3 stdout must clamp to the last output"
        );

        assert_eq!(
            counting_stub_invocations(dir.path(), "counting-stub.sh"),
            3,
            "invocation counter must read exactly 3 after three runs"
        );
    }
}
