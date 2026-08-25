//! Phase 50.1 Plan 03 (D-04 / D-05 / D-06 / D-13 / D-20): process-isolated bot
//! message dispatch.
//!
//! **Security contract** (mirrors `crates/ironhermes-kanban/src/worker_spawn.rs`'s
//! own module doc, the production-proven shape D-05 requires this path to
//! reuse):
//!
//! 1. **Env scrub** ([`build_bot_handoff_env`]) — builds the exact allowlisted
//!    system vars the child subprocess should receive. Everything else in the
//!    parent process's environment is DROPPED. This is the security guarantee
//!    that stops an operator's shell secrets from reaching a bot subprocess
//!    (T-50.1-03-01).
//! 2. **Subprocess spawn** ([`run_bot_handoff`]) — calls `.env_clear()` BEFORE
//!    `.envs(...)` on the `Command` builder, sets the working directory via
//!    `.current_dir(...)` (never a `--in <path>` argv token — RESEARCH.md
//!    Pitfall 2: `ironhermes-cli`'s `chat` subcommand defines only a
//!    positional message, `-q/--query` and `--yolo`), and captures
//!    stdout/stderr instead of inheriting the parent's.
//!
//! **The live bot is never reached through this path (D-04).** Only a
//! non-live bot's traffic goes through CLI handoff; the live profile streams
//! through the embedded `global_app_state()` runtime via the Chat screen.
//! [`run_bot_handoff`] guards this explicitly before any spawn.
//!
//! **D-13 preview write-back.** On a successful dispatch, [`run_bot_handoff`]
//! writes the captured reply back into the bot-meta store
//! (`crate::server::bot_meta_api::write_preview_for`) from THIS process's own
//! captured child output — never a cross-profile database read
//! (T-50.1-03-08). A store-write failure is logged and does not fail an
//! otherwise-successful dispatch; a failed dispatch never calls the
//! write-back at all, so an existing preview is never clobbered with an
//! empty value. **OF-9 (gap task):** the text used for this write-back is
//! ADDITIONALLY run through [`strip_think_blocks`] — a `<think>…</think>`
//! reasoning preamble never becomes the preview. A reply that is ENTIRELY a
//! think block skips the write-back the same way a log-only reply skips it,
//! but (unlike the log-only case) is NOT a dispatch failure — the reply
//! delivered to the caller keeps the full text.
//!
//! **D-20 reuse contract.** [`run_bot_handoff`] is a synchronous,
//! `#[cfg(feature = "server")]`-only spawn-wait-capture function with no
//! dependency on the Dioxus server-fn machinery — Phase 50.2's per-round
//! group-chat member-turn driver can call it directly, exactly as this
//! plan's own [`dispatch_bot_message`] does, without re-deriving any of the
//! isolation logic above.
//!
//! **Allowlist duplication is deliberate.** [`BOT_HANDOFF_SAFE_SYSTEM_VARS`]
//! is a copy of `ironhermes_kanban::worker_spawn::SAFE_SYSTEM_VARS`'s eight
//! entries, not an import across the crate fence — `ironhermes-kanban` is a
//! crate this UI server already depends on, but importing a kanban-private
//! constant here would couple this path's security allowlist to a crate
//! whose own module doc scopes it to kanban worker spawn specifically. Both
//! copies are security-relevant and MUST change together if the allowlist
//! ever changes.
//!
//! **Never used as the profile boundary (D-07).** The in-process subagent
//! delegation tool (`ironhermes_tools`'s in-process "delegate task" helper)
//! spawns children on the parent's OWN profile via `tokio::spawn` — it is
//! explicitly not a process boundary and is not referenced anywhere in this
//! file (see `50.1-CONTEXT.md` D-07).

use dioxus::prelude::*;

#[cfg(feature = "server")]
use std::collections::BTreeMap;
#[cfg(feature = "server")]
use std::path::{Path, PathBuf};
#[cfg(feature = "server")]
use std::process::Stdio;

/// Phase 50.1 Plan 03 (D-05): the eight-entry safe-system-vars allowlist for
/// the bot handoff path — a deliberate duplication of
/// `ironhermes_kanban::worker_spawn::SAFE_SYSTEM_VARS` (see module doc). The
/// worker-binary override variable (`IRONHERMES_WORKER_BIN`) rides this
/// allowlist so a recursive spawn (bot handoff -> further subprocess) would
/// carry the override forward, mirroring kanban's own forward-compat note.
#[cfg(feature = "server")]
pub(crate) const BOT_HANDOFF_SAFE_SYSTEM_VARS: &[&str] = &[
    "PATH",
    "HOME",
    "USER",
    "LANG",
    "TERM",
    "RUST_LOG",
    "IRONHERMES_HOME",
    "IRONHERMES_WORKER_BIN",
];

/// Phase 50.1 Plan 03 (T-50.1-03-04): bounded wait for the bot subprocess. A
/// chat turn may involve several tool calls, so this is generous relative to
/// `profile_verify_api::PROBE_TIMEOUT_SECONDS` (20s, a health probe only) —
/// but still bounded, so a hung child cannot pin the blocking pool
/// indefinitely.
#[cfg(feature = "server")]
const BOT_HANDOFF_TIMEOUT_SECONDS: u64 = 180;

/// Phase 50.1 Plan 03: every failure mode on the handoff path, named so each
/// one arrives as an operator-readable message rather than a panic or an
/// unhandled task death. No variant's `Display` embeds the child's raw
/// environment, a key value, or a raw line from any `.env` file — the exact
/// mechanism behind this project's CR-05/CR-06 disclosures.
#[cfg(feature = "server")]
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum BotHandoffError {
    /// `bot_name` failed `ironhermes_core::profile::validate_profile_name`.
    InvalidProfileName { name: String, reason: String },
    /// The resolved workspace path failed validation (empty, relative, or
    /// contains an unexpanded shell variable) before any filesystem access.
    WorkspacePathRejected { path: String, reason: String },
    /// The resolved workspace path does not exist or is not a directory.
    WorkspaceNotFound { path: PathBuf },
    /// `IRONHERMES_WORKER_BIN` was set to an empty/whitespace-only value.
    WorkerBinaryNotResolvable,
    /// `Command::spawn()` itself failed (binary not found, permission
    /// denied, etc).
    SpawnFailed { reason: String },
    /// The child exited with a non-zero, non-key-related status.
    NonZeroExit { code: i32 },
    /// The child did not finish within [`BOT_HANDOFF_TIMEOUT_SECONDS`] and
    /// was killed.
    Timeout { seconds: u64 },
    /// The target profile has no resolvable key for its configured
    /// provider. Carries the profile name and the missing key's NAME only —
    /// never a value, never raw child output, never a raw `.env` line. This
    /// is the named-error path for the exact failure class that has already
    /// crashed `build_runtime_judge_fn` on the kanban worker path in this
    /// project (a missing profile key).
    MissingProviderKey { profile: String, key_name: String },
    /// `bot_name` is the process's live profile — it streams through the
    /// Chat screen and must never be reached by a subprocess (D-04).
    LiveProfileMustStream,
    /// OF-5 (gap task): the child exited successfully, but every byte of
    /// its captured stdout was ANSI-escaped tracing/log noise —
    /// [`sanitize_reply`] filtered it all out. This is the named failure
    /// path OF-5 requires: a log line must never masquerade as a reply, so
    /// this is surfaced as an explicit error rather than a silently empty
    /// or (worse) log-polluted reply string. Carries no raw child output —
    /// only the byte count, for operator-readable diagnostics.
    ReplyWasLogOnly { raw_byte_count: usize },
    /// Phase 50.2 Plan 14 (G-50.2-2b): the operator cancelled this turn
    /// through the In-Flight Turns panel's CANCEL control before the child
    /// finished. `spawn_and_capture` observed the token, killed and reaped
    /// the child, and returned this named error — never a `Timeout` or a
    /// generic `NonZeroExit`, so the room transcript can record it as this
    /// member's distinguishable cancelled turn (D-20's failure-as-data
    /// contract).
    Cancelled,
}

#[cfg(feature = "server")]
impl std::fmt::Display for BotHandoffError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidProfileName { name, reason } => {
                write!(f, "\"{name}\" is not a valid bot name: {reason}")
            }
            Self::WorkspacePathRejected { path, reason } => {
                write!(f, "bot workspace path rejected ({path}): {reason}")
            }
            Self::WorkspaceNotFound { path } => {
                write!(
                    f,
                    "bot workspace not found or not a directory: {}",
                    path.display()
                )
            }
            Self::WorkerBinaryNotResolvable => {
                write!(f, "the ironhermes worker binary could not be resolved")
            }
            Self::SpawnFailed { reason } => {
                write!(f, "failed to spawn the bot subprocess: {reason}")
            }
            Self::NonZeroExit { code } => write!(f, "bot subprocess exited with code {code}"),
            Self::Timeout { seconds } => write!(
                f,
                "bot subprocess did not finish within {seconds}s and was terminated"
            ),
            Self::MissingProviderKey { profile, key_name } => write!(
                f,
                "profile \"{profile}\" is missing its {key_name} — add it in the bot's Advanced pane before messaging this bot"
            ),
            Self::LiveProfileMustStream => write!(
                f,
                "this bot is the live profile — use CHAT to open the streaming Chat screen instead of sending a one-shot message"
            ),
            Self::ReplyWasLogOnly { raw_byte_count } => write!(
                f,
                "the bot subprocess produced no reply text — only {raw_byte_count} bytes of log/tracing output were captured"
            ),
            Self::Cancelled => {
                write!(f, "the bot turn was cancelled by the operator")
            }
        }
    }
}

#[cfg(feature = "server")]
impl std::error::Error for BotHandoffError {}

/// Phase 50.1 Plan 03 (D-02): resolve the ironhermes worker binary. Mirrors
/// `ironhermes_kanban::worker_spawn::resolve_worker_bin` exactly — reads
/// `IRONHERMES_WORKER_BIN` first (worktree `cargo run` pin), falls back to
/// `"ironhermes"` (PATH lookup). Unlike the kanban original this returns a
/// `Result`: an explicitly-set-but-empty override is a real misconfiguration
/// this path names rather than silently spawning an empty-string command.
#[cfg(feature = "server")]
pub(crate) fn resolve_bot_worker_bin() -> Result<String, BotHandoffError> {
    let bin = std::env::var("IRONHERMES_WORKER_BIN").unwrap_or_else(|_| "ironhermes".to_string());
    if bin.trim().is_empty() {
        return Err(BotHandoffError::WorkerBinaryNotResolvable);
    }
    Ok(bin)
}

/// Phase 50.1 Plan 03 (RESEARCH Pitfall 2) / Phase 50.2 Plan 02 (D-21): pure
/// argv builder — no process access, so its whole contract is unit-testable
/// without spawning anything. `session_title: None` emits exactly
/// `["--profile", bot_name, "chat", "-q", message]` — the pre-50.2 vector,
/// unchanged. `Some(t)` appends two further elements, `"--session"` and `t`,
/// so the child's own `chat -q --session <title>` (Phase 50.2 Plan 02) can
/// resolve-or-create its persistent session by that title.
/// `ironhermes-cli`'s `chat` subcommand today accepts a positional message,
/// `-q/--query`, `--yolo`, and `--session <title>` — any other token (e.g. a
/// literal `--in <path>`) would still be rejected by the argument parser at
/// runtime, so the working directory is set on the process builder instead
/// ([`run_bot_handoff`]), never carried in this vector.
#[cfg(feature = "server")]
pub(crate) fn build_bot_handoff_argv(
    bot_name: &str,
    message: &str,
    session_title: Option<&str>,
) -> Vec<String> {
    let mut argv = vec![
        "--profile".to_string(),
        bot_name.to_string(),
        "chat".to_string(),
        "-q".to_string(),
        message.to_string(),
    ];
    if let Some(title) = session_title {
        argv.push("--session".to_string());
        argv.push(title.to_string());
    }
    argv
}

/// Phase 50.1 Plan 03 (D-05 / T-50.1-03-01): builds the COMPLETE environment
/// the child receives — the allowlisted parent variables that are actually
/// present, plus `IRONHERMES_HOME` always pinned to the resolved hermes home
/// (so the child resolves the target profile even when the parent process
/// never had `IRONHERMES_HOME` set as an OS env var and relies on
/// `get_hermes_home()`'s own default resolution) — and nothing else.
/// Everything not in this map is dropped when the caller calls
/// `.env_clear()` before `.envs(...)`; that is the security guarantee.
#[cfg(feature = "server")]
pub(crate) fn build_bot_handoff_env() -> BTreeMap<String, String> {
    let mut env = BTreeMap::new();
    for var in BOT_HANDOFF_SAFE_SYSTEM_VARS {
        if let Ok(v) = std::env::var(var) {
            env.insert((*var).to_string(), v);
        }
    }
    env.insert(
        "IRONHERMES_HOME".to_string(),
        ironhermes_core::get_hermes_home()
            .to_string_lossy()
            .into_owned(),
    );
    // OF-5 (50.1-OPERATOR-FEEDBACK.md, gap task): `ironhermes-cli`'s
    // non-interactive `chat -q` path (`main.rs`'s `is_interactive_repl`
    // gate) is classified NON-interactive, so when RUST_LOG is unset it
    // falls to the `ironhermes=info` default directive — and the
    // `tracing_subscriber::fmt()` subscriber installed for that path
    // writes to STDOUT (only the `acp` subcommand redirects to stderr).
    // That is the exact root cause of the ANSI/log-garbled roster preview
    // the operator reported: INFO-level tracing lines interleave with the
    // streamed reply text on the SAME stream this fn's caller captures as
    // "the reply". Forcing "off" here — deliberately overwriting whatever
    // the loop above may have copied from the allowlisted parent
    // `RUST_LOG` — is orchestrator-controlled: this is a headless
    // machine-to-machine dispatch, never an operator's interactive debug
    // session, so the operator's own shell `RUST_LOG` preference must
    // never leak into (and corrupt) a captured bot reply. `sanitize_reply`
    // below is the belt-and-braces second layer for any output this
    // primary fix does not eliminate (e.g. a future log call this
    // override doesn't anticipate).
    env.insert("RUST_LOG".to_string(), "off".to_string());
    env
}

/// Phase 50.1 Plan 03 (D-06): resolve a validated, canonical, absolute
/// working directory for the bot subprocess. Mirrors
/// `ironhermes_kanban::worker_spawn::resolve_workspace_dir`'s validation
/// shape: reject empty, reject a shell-expansion sigil, reject a
/// non-absolute path, reject a path that is not a directory, then
/// canonicalize. `bot_name` is validated via
/// `ironhermes_core::profile::validate_profile_name` before it is ever
/// joined into a path. When `override_path` is `None`, defaults to the
/// target profile's own `workspace/` subdirectory beneath the profiles root
/// — every profile directory already carries one (RESEARCH.md, verified on
/// the live filesystem). Unlike kanban's scratch-workspace case, a missing
/// default workspace dir is a named error here, never auto-created — D-06
/// leaves per-bot workspace *provisioning* to a later phase.
#[cfg(feature = "server")]
pub(crate) fn resolve_bot_workspace_dir(
    bot_name: &str,
    override_path: Option<&str>,
) -> Result<PathBuf, BotHandoffError> {
    let validated_name =
        ironhermes_core::profile::validate_profile_name(bot_name).map_err(|e| {
            BotHandoffError::InvalidProfileName {
                name: bot_name.to_string(),
                reason: e.to_string(),
            }
        })?;

    let raw: String = match override_path {
        Some(p) => p.to_string(),
        None => crate::server::profile_api::profile_dir_for(&validated_name)
            .join("workspace")
            .to_string_lossy()
            .into_owned(),
    };

    if raw.is_empty() {
        return Err(BotHandoffError::WorkspacePathRejected {
            path: raw,
            reason: "empty workspace path".to_string(),
        });
    }
    if raw.contains('$') {
        return Err(BotHandoffError::WorkspacePathRejected {
            path: raw,
            reason: "contains an unexpanded shell variable '$'".to_string(),
        });
    }
    let ws_path = Path::new(&raw);
    if !ws_path.is_absolute() {
        return Err(BotHandoffError::WorkspacePathRejected {
            path: raw,
            reason: "not an absolute path".to_string(),
        });
    }
    if !ws_path.is_dir() {
        return Err(BotHandoffError::WorkspaceNotFound {
            path: ws_path.to_path_buf(),
        });
    }
    std::fs::canonicalize(ws_path).map_err(|e| BotHandoffError::WorkspacePathRejected {
        path: raw.clone(),
        reason: format!("canonicalize failed: {e}"),
    })
}

/// Phase 50.1 Plan 03: current wall-clock time in milliseconds. Duplicated
/// from `bot_meta_api::now_ms` (module-private there) rather than widening
/// its visibility — same "duplicate the trivial helper" precedent
/// `bot_meta_api.rs`'s own test module already documents for `ScopedEnv`.
#[cfg(feature = "server")]
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Phase 50.1 Plan 03: best-effort classification of a non-zero child exit
/// as "missing provider key" versus a generic failure. Scans only for a
/// small set of known markers (never quotes the raw stderr back into an
/// error message — that would risk exactly the CR-05/CR-06 leak class this
/// module's error variants are designed to avoid).
///
/// MI-02 fix (`50.1-REVIEW.md`): the bare digit sequence `"401"` used to be
/// its own marker, so any unrelated failure whose stderr happened to
/// contain `"4010"`, a line number like `":401:"`, or a port/id containing
/// `401` was misclassified as a missing key — an actionable-looking message
/// telling the operator to add a key that already exists. `"401"` alone is
/// dropped; `"401 unauthorized"` (the shape a real HTTP 401 status line
/// takes) is a strictly more specific replacement, and `"unauthorized"`
/// already covers phrasings that spell the word without the numeric code
/// adjacent to it.
#[cfg(feature = "server")]
fn stderr_indicates_missing_key(stderr: &[u8]) -> bool {
    const MARKERS: &[&str] = &[
        "401 unauthorized",
        "unauthorized",
        "no key for that provider resolves",
        "no api key",
        "missing api key",
        "api key required",
    ];
    let text = String::from_utf8_lossy(stderr).to_lowercase();
    MARKERS.iter().any(|m| text.contains(m))
}

/// Phase 50.1 Plan 03: best-effort resolution of the env-var NAME a target
/// profile's configured provider expects, for [`BotHandoffError::MissingProviderKey`]'s
/// `key_name` field. Reads the target profile's own `config.yaml` directly
/// (mirrors `ironhermes_core::dispatch_gate::evaluate_profile_dispatch_at`'s
/// own direct `Config::load_from` read of a non-live profile) rather than
/// the running process's own config. `None` on any failure — the caller
/// falls back to a generic label rather than surfacing an internal error
/// here.
#[cfg(feature = "server")]
fn resolve_expected_key_name(bot_name: &str) -> Option<String> {
    let config_path = crate::server::profile_api::profile_dir_for(bot_name).join("config.yaml");
    let config = ironhermes_core::config::Config::load_from(&config_path).ok()?;
    let provider = config.model.provider.clone();
    crate::server::profile_api::provider_key_env_name_for(&config, &provider)
}

/// OF-5 (gap task): strip ANSI/VT100 escape sequences from `input`. Handles
/// the CSI form (`ESC '[' ... final-byte`, e.g. `\x1b[32m`) —
/// `tracing_subscriber::fmt()`'s default ANSI-colored output (the exact
/// shape the operator's evidence screenshot showed leaking into a captured
/// reply) never emits anything else. A bare/non-CSI `ESC` byte is dropped on
/// its own rather than echoed — this fn's contract is "the output is safe to
/// treat as plain text", not "byte-for-byte passthrough of unrecognized
/// escapes".
#[cfg(feature = "server")]
fn strip_ansi_escapes(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        // Peek without consuming twice: clone the iterator so a non-'['
        // escape byte is simply dropped (not the character following it).
        let mut lookahead = chars.clone();
        if lookahead.next() == Some('[') {
            chars = lookahead; // consume the '['
            // CSI grammar (ECMA-48): parameter bytes 0x30-0x3F, intermediate
            // bytes 0x20-0x2F, terminated by one final byte 0x40-0x7E.
            for next in chars.by_ref() {
                if ('@'..='~').contains(&next) {
                    break;
                }
            }
        }
        // else: bare ESC, already dropped by not pushing it.
    }
    out
}

/// OF-5 (gap task): `true` when `line` matches `ironhermes-cli`'s own
/// `tracing_subscriber::fmt()` timestamp prefix
/// (`YYYY-MM-DDTHH:MM:SS...Z  LEVEL ...`, `.with_target(false)` so no
/// module path follows) — the exact shape the non-interactive `chat -q`
/// path emits to stdout absent this module's `RUST_LOG=off` override (see
/// `build_bot_handoff_env`'s doc comment for the root-cause chain). Matched
/// on the ISO-8601 timestamp alone (no level-keyword requirement) so this
/// stays robust to future field/level changes in that format; false-positive
/// risk is accepted as negligible — a reply beginning a line with exactly
/// this 19-character timestamp shape is not a pattern real assistant text
/// produces.
#[cfg(feature = "server")]
fn looks_like_tracing_log_line(line: &str) -> bool {
    let bytes = line.trim_start().as_bytes();
    if bytes.len() < 19 {
        return false;
    }
    let digit = |i: usize| bytes[i].is_ascii_digit();
    (0..4).all(digit)
        && bytes[4] == b'-'
        && (5..7).all(digit)
        && bytes[7] == b'-'
        && (8..10).all(digit)
        && bytes[10] == b'T'
        && (11..13).all(digit)
        && bytes[13] == b':'
        && (14..16).all(digit)
        && bytes[16] == b':'
        && (17..19).all(digit)
}

/// Phase 50.1 OF-5 (gap task): sanitizes a captured child stdout buffer into
/// the bot's actual reply text — the belt-and-braces second layer
/// (`build_bot_handoff_env`'s `RUST_LOG=off` is the primary fix) that ALWAYS
/// runs regardless of whether the primary fix eliminated every log line.
/// ANSI escapes are stripped first (so a log line's color codes never hide
/// it from the next step), then every remaining line matching
/// [`looks_like_tracing_log_line`] is dropped. When the raw input carried
/// real content but sanitization removed all of it — the exact failure OF-5
/// evidenced, a captured "reply" that was 100% subprocess log noise — this
/// returns [`BotHandoffError::ReplyWasLogOnly`] rather than a log-polluted
/// or silently empty reply. A raw input that was already empty/whitespace
/// stays a legitimate empty reply (unchanged pre-OF-5 behavior).
#[cfg(feature = "server")]
fn sanitize_reply(raw: &[u8]) -> Result<String, BotHandoffError> {
    let text = String::from_utf8_lossy(raw);
    let had_raw_content = !text.trim().is_empty();
    let stripped = strip_ansi_escapes(&text);
    let cleaned: String = stripped
        .lines()
        .filter(|line| !looks_like_tracing_log_line(line))
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string();
    if had_raw_content && cleaned.is_empty() {
        return Err(BotHandoffError::ReplyWasLogOnly {
            raw_byte_count: raw.len(),
        });
    }
    Ok(cleaned)
}

/// Phase 50.1 OF-9 (gap task): strips every `<think>…</think>` block from
/// `input` — a reasoning model's preamble, which the operator's evidence
/// showed leaking verbatim into a roster card's D-13 preview (a reply
/// literally rendering `<think>` before any real content). Handles an
/// arbitrary number of blocks (each iteration searches for the NEXT
/// `<think>`) and the unclosed-leading-tag case: a `<think>` with no
/// matching `</think>` strips to the end of the string rather than being
/// left dangling — the same "degrade to nothing rather than half-render a
/// raw tag" posture `sanitize_reply` already takes for log noise. Pure
/// string logic, deliberately NOT folded into `sanitize_reply` itself —
/// [`run_bot_handoff`] applies this only to the COPY of the reply used for
/// the D-13 preview; the reply delivered to the caller (and shown in the
/// floating chat window) keeps the full text, and the chat window is
/// responsible for its own render-time hiding (module doc,
/// `bot_roster/chat_window.rs`) so a think-only reply is still visibly
/// "delivered" there rather than silently vanishing.
///
/// Phase 50.2 Plan 11 (G-3): `pub(crate)` so `group_chat_store.rs`'s
/// `write_room_preview` can strip a room's preview line before it is
/// PERSISTED pre-truncated. The "each surface strips at render time"
/// rationale above holds for the reply value delivered to a live caller —
/// it does not hold for a value written to disk and rendered verbatim by
/// a later surface with no strip opportunity of its own
/// (`bot_roster/group_row.rs`).
#[cfg(feature = "server")]
pub(crate) fn strip_think_blocks(input: &str) -> String {
    const OPEN: &str = "<think>";
    const CLOSE: &str = "</think>";
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    loop {
        match rest.find(OPEN) {
            None => {
                out.push_str(rest);
                break;
            }
            Some(open_idx) => {
                out.push_str(&rest[..open_idx]);
                let after_open = &rest[open_idx + OPEN.len()..];
                match after_open.find(CLOSE) {
                    Some(close_idx) => {
                        rest = &after_open[close_idx + CLOSE.len()..];
                    }
                    None => {
                        // Unclosed leading tag: everything from here to the
                        // end of the string is dropped, deliberately — never
                        // echoed as a dangling raw tag.
                        break;
                    }
                }
            }
        }
    }
    out
}

/// Phase 50.1 Plan 03 (D-05): spawn `worker_bin argv...` with the environment
/// scrubbed to exactly `env`, capture its exit status and both output
/// streams, bounded by [`BOT_HANDOFF_TIMEOUT_SECONDS`]. Reader threads drain
/// stdout/stderr concurrently with the wait loop so a chatty child cannot
/// deadlock on a full OS pipe buffer while this fn polls `try_wait()`.
///
/// Phase 50.2 Plan 14 (G-50.2-2b): `cancel` is an OPTIONAL cooperative
/// cancellation handle. `None` (every pre-50.2 caller) behaves byte-for-byte
/// as before — success, non-zero exit and the 180s timeout are unchanged.
/// `Some(token)` is polled synchronously (`CancellationToken::is_cancelled`
/// never awaits) on every `try_wait()` iteration, BEFORE the elapsed-versus-
/// timeout check, so a cancellation observed at 300ms returns in well under
/// a second rather than waiting out the full ceiling. No `await`, no
/// `block_on` and no `block_in_place` is introduced by this check — this
/// function remains a fully synchronous, blocking fn.
#[cfg(feature = "server")]
fn spawn_and_capture(
    worker_bin: &str,
    argv: &[String],
    workspace: &Path,
    env: &BTreeMap<String, String>,
    cancel: Option<&tokio_util::sync::CancellationToken>,
) -> Result<(std::process::ExitStatus, Vec<u8>, Vec<u8>), BotHandoffError> {
    // CRITICAL: .env_clear() BEFORE .envs(...) — no inherited shell secret
    // reaches the bot subprocess (D-05 / T-50.1-03-01). Mirrors
    // worker_spawn.rs:573-601's ordering exactly.
    let mut child = std::process::Command::new(worker_bin)
        .args(argv)
        .current_dir(workspace)
        .env_clear()
        .envs(env)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| BotHandoffError::SpawnFailed {
            reason: e.to_string(),
        })?;

    let stdout_pipe = child.stdout.take();
    let stderr_pipe = child.stderr.take();

    let stdout_handle = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(mut pipe) = stdout_pipe {
            let _ = std::io::Read::read_to_end(&mut pipe, &mut buf);
        }
        buf
    });
    let stderr_handle = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(mut pipe) = stderr_pipe {
            let _ = std::io::Read::read_to_end(&mut pipe, &mut buf);
        }
        buf
    });

    let timeout = std::time::Duration::from_secs(BOT_HANDOFF_TIMEOUT_SECONDS);
    let start = std::time::Instant::now();
    let status_result = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) => {
                // Phase 50.2 Plan 14 (G-50.2-2b): checked BEFORE the
                // elapsed-versus-timeout check, so a cancelled turn returns
                // promptly rather than waiting out the 180s ceiling.
                // `is_cancelled()` is synchronous — no await introduced.
                if cancel.is_some_and(|t| t.is_cancelled()) {
                    let _ = child.kill();
                    let _ = child.wait();
                    break Err(BotHandoffError::Cancelled);
                }
                if start.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    break Err(BotHandoffError::Timeout {
                        seconds: BOT_HANDOFF_TIMEOUT_SECONDS,
                    });
                }
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
            Err(e) => {
                // MI-01 fix (`50.1-REVIEW.md`): mirror the timeout branch
                // above — a `try_wait` error must not leave the child
                // running unsupervised and unreaped while the operator is
                // told the dispatch failed.
                let _ = child.kill();
                let _ = child.wait();
                break Err(BotHandoffError::SpawnFailed {
                    reason: e.to_string(),
                });
            }
        }
    };
    let status = status_result?;

    let stdout = stdout_handle.join().unwrap_or_default();
    let stderr = stderr_handle.join().unwrap_or_default();
    Ok((status, stdout, stderr))
}

/// Phase 50.1 Plan 03 (D-04 / D-05 / D-06 / D-13 / D-20): the blocking
/// spawn-wait-capture-and-write-back function both this plan's
/// [`dispatch_bot_message`] and Phase 50.2's group-chat round driver call
/// directly. Guards the live profile first (no spawn at all for the live
/// bot — D-04), resolves the workspace and worker binary, builds the argv
/// and environment, spawns and waits bounded, then classifies the result:
/// success writes the D-13 preview back (best-effort; a store-write failure
/// is logged, not surfaced as a dispatch failure) and returns the reply;
/// non-zero exit is classified as a missing-provider-key error when the
/// child's stderr indicates one, otherwise a generic non-zero-exit error.
/// Never opens any profile's on-disk agent database (D-13/D-23) — the reply
/// comes strictly from this process's own captured child output.
///
/// Phase 50.2 Plan 14 (G-50.2-2b): no production call site reaches this fn
/// directly any more — `dispatch_bot_message` and `dispatch_member_turn`
/// both route through the tracked seam
/// (`run_bot_handoff_tracked`/`run_bot_handoff_tracked_in`) so a subprocess
/// turn registers in the `TurnRegistry`. This four-argument signature is
/// kept, unchanged, as the interface every pre-existing test in this module
/// calls, and is pinned against regression by
/// `run_bot_handoff_with_cancel_none_matches_run_bot_handoff` (Task 1) —
/// hence `#[allow(dead_code)]` rather than deletion.
#[cfg_attr(not(test), allow(dead_code))]
#[cfg(feature = "server")]
pub(crate) fn run_bot_handoff(
    bot_name: &str,
    message: &str,
    override_workspace: Option<&str>,
    session_title: Option<&str>,
) -> Result<crate::protocol::BotHandoffResult, BotHandoffError> {
    run_bot_handoff_with_cancel(bot_name, message, override_workspace, session_title, None)
}

/// Phase 50.2 Plan 14 (G-50.2-2b): the same spawn-wait-capture-and-write-back
/// body [`run_bot_handoff`] has always had, with one addition — an optional
/// [`tokio_util::sync::CancellationToken`] threaded into [`spawn_and_capture`]
/// so a caller that registered this turn in the process-wide `TurnRegistry`
/// (Task 2) can cancel it cooperatively. `cancel: None` is byte-for-byte
/// [`run_bot_handoff`]'s pre-existing behaviour — that four-argument fn is
/// now a one-line delegation to this one. A cancelled turn's `Cancelled`
/// error returns through the same `?` [`BotHandoffError::Timeout`] already
/// used, which is what keeps the D-13 preview write-back below from ever
/// running on a cancelled turn.
#[cfg(feature = "server")]
pub(crate) fn run_bot_handoff_with_cancel(
    bot_name: &str,
    message: &str,
    override_workspace: Option<&str>,
    session_title: Option<&str>,
    cancel: Option<&tokio_util::sync::CancellationToken>,
) -> Result<crate::protocol::BotHandoffResult, BotHandoffError> {
    let validated_name =
        ironhermes_core::profile::validate_profile_name(bot_name).map_err(|e| {
            BotHandoffError::InvalidProfileName {
                name: bot_name.to_string(),
                reason: e.to_string(),
            }
        })?;

    // D-04: the live bot streams through the embedded runtime, reached via
    // the Chat screen — never a subprocess. Live identity is resolved via
    // `ironhermes_core::current_profile()` directly rather than widening
    // `crate::server::api::active_profile_identifier`'s visibility (that fn
    // is module-private in api.rs; `bot_meta_api::live_profile_name`
    // already established this precedent — same canonical algorithm,
    // `server/api.rs` stays untouched by this plan too).
    if validated_name == ironhermes_core::current_profile() {
        return Err(BotHandoffError::LiveProfileMustStream);
    }

    let workspace = resolve_bot_workspace_dir(&validated_name, override_workspace)?;

    // OF-6 fix (`50.1-OPERATOR-FEEDBACK.md`, gap task): self-heal a persona
    // saved before this fix shipped (at the legacy `workspace/SOUL.md`
    // location Plan 05 originally used) into the canonical profile-root
    // `SOUL.md` the agent runtime actually reads at turn time — before
    // every dispatch, so an already-configured bot (e.g. zig) loads its
    // persona on the very next turn without the operator re-typing it.
    // Best-effort and infallible by construction (never returns an error);
    // a migration miss just means this turn falls back to the default
    // identity, exactly today's pre-fix behavior, never a broken dispatch.
    crate::server::profile_api::migrate_legacy_persona_if_needed(&validated_name);

    let worker_bin = resolve_bot_worker_bin()?;
    let argv = build_bot_handoff_argv(&validated_name, message, session_title);
    let env = build_bot_handoff_env();

    let start = std::time::Instant::now();
    let (status, stdout, stderr) =
        spawn_and_capture(&worker_bin, &argv, &workspace, &env, cancel)?;
    let elapsed_ms = start.elapsed().as_millis() as u64;

    if status.success() {
        // OF-5 (gap task): ANSI-stripped, log-line-filtered reply text —
        // never the raw captured bytes. See `sanitize_reply`'s doc comment
        // for the full root-cause chain and the named-error path this
        // guards (a `?` here means a log-only stdout surfaces as
        // `BotHandoffError::ReplyWasLogOnly` and skips the preview
        // write-back below entirely, exactly like any other dispatch
        // failure — an existing preview is never clobbered).
        let reply = sanitize_reply(&stdout)?;
        // OF-9 (gap task): the D-13 preview must never show a reasoning
        // model's <think> preamble as if it were the reply itself (operator
        // evidence: a preview literally rendering "<think>"). This strips a
        // COPY used only for the preview text — `reply` below (delivered to
        // the caller / chat window) keeps the full text, think tags intact
        // (`strip_think_blocks`'s own doc comment).
        let preview_text = strip_think_blocks(&reply);
        // D-13: write the reply preview back on completion, from THIS
        // process's own captured child output — never a cross-profile
        // database read (T-50.1-03-08). Best-effort: a store-write failure
        // must not fail an otherwise-successful dispatch.
        if preview_text.trim().is_empty() {
            // OF-9: a reply that is ONLY a think block must not clobber (or
            // blank out) an existing preview with empty-string "content".
            // Mirrors the ReplyWasLogOnly discipline of "no write-back call
            // at all" — but unlike that error path, this is NOT a dispatch
            // failure: `reply` is still delivered to the caller below.
            tracing::debug!(
                bot = %validated_name,
                "reply was entirely a <think> block — skipping D-13 preview write-back"
            );
        } else if let Err(e) =
            crate::server::bot_meta_api::write_preview_for(&validated_name, &preview_text, now_ms())
        {
            tracing::warn!(
                bot = %validated_name,
                error = %e,
                "failed to write bot handoff preview back to bot-meta store"
            );
        }
        Ok(crate::protocol::BotHandoffResult {
            reply,
            exit_ok: true,
            error: None,
            elapsed_ms,
        })
    } else if stderr_indicates_missing_key(&stderr) {
        let key_name = resolve_expected_key_name(&validated_name)
            .unwrap_or_else(|| "provider API key".to_string());
        Err(BotHandoffError::MissingProviderKey {
            profile: validated_name,
            key_name,
        })
    } else {
        let code = status.code().unwrap_or(-1);
        Err(BotHandoffError::NonZeroExit { code })
    }
}

/// Phase 50.2 Plan 14 (G-50.2-2b): pure, I/O-free render of the label the
/// In-Flight Turns panel's `TurnCard` shows for a subprocess-transport turn
/// — the `TurnEntry.session_id` string a handoff turn registers under, since
/// no new field is added to `TurnEntry`/`TurnSummary` (this plan's own
/// prohibition). With a non-empty-after-trim `session_title`, renders the
/// bot's name, a space-padded middle dot, then the title — so a group-room
/// member turn reads as `"<member> · Group: <room name>"` and a Bot Chat
/// turn reads as `"<bot> · Bot Chat"`. With no title (or a whitespace-only
/// one), renders the bare bot name. Never empty: `bot_name` is always a
/// validated, non-empty profile name by the time a caller reaches this fn.
#[cfg(feature = "server")]
pub(crate) fn handoff_turn_session_id(bot_name: &str, session_title: Option<&str>) -> String {
    let name = bot_name.trim();
    match session_title.map(str::trim).filter(|t| !t.is_empty()) {
        Some(title) => format!("{name} \u{b7} {title}"),
        None => name.to_string(),
    }
}

/// Phase 50.2 Plan 14 (G-50.2-2b): resolve the process-wide `TurnRegistry`
/// this crate's web panel already reads (`AppState.turn_registry`) — or a
/// process-local fallback when `AppState` is not yet initialised.
///
/// In production the web server always initialises `AppState` before any
/// server fn can run, so this reaches [`crate::server::state::try_global_app_state`]'s
/// `Some` branch and every registration really does write into the SAME map
/// `turns_list()` reads (cloning a `TurnRegistry` yields a second handle to
/// the same inner map). The fallback branch exists ONLY for the in-file test
/// modules in this file that never initialise `AppState` — a handoff must
/// never be blocked, and must never panic, because the panel that observes
/// it is absent.
#[cfg(feature = "server")]
pub(crate) fn handoff_turn_registry() -> ironhermes_core::TurnRegistry {
    if let Some(state) = crate::server::state::try_global_app_state() {
        return (*state.turn_registry).clone();
    }
    static FALLBACK: std::sync::OnceLock<ironhermes_core::TurnRegistry> = std::sync::OnceLock::new();
    FALLBACK
        .get_or_init(ironhermes_core::TurnRegistry::new)
        .clone()
}

/// Phase 50.2 Plan 14 (G-50.2-2b): RAII guard that deregisters a
/// `TurnRegistry` entry on drop. `Drop` is synchronous and
/// `TurnRegistry::deregister` is async, so the drop impl detaches a
/// `tokio::spawn` rather than awaiting directly — this is what makes
/// deregistration fire on every exit path (success, error, early return, or
/// the surrounding task itself being cancelled), not just the path that
/// reaches the end of the wrapper function.
///
/// Phase 50.2 Plan 19 (CR-01): "the surrounding task" is, at every
/// production call site, the [`await_detached`]-spawned task the tracked
/// call now runs on — never the one-shot `#[server]` fn's own request
/// future. That is the whole point of the seam: this guard's drop still
/// fires exactly the same way when its OWN task is cancelled, but the HTTP
/// request future going away no longer reaches it at all.
#[cfg(feature = "server")]
struct DeregisterOnDrop {
    registry: ironhermes_core::TurnRegistry,
    turn_id: ironhermes_core::TurnId,
}

#[cfg(feature = "server")]
impl Drop for DeregisterOnDrop {
    fn drop(&mut self) {
        let registry = self.registry.clone();
        let turn_id = self.turn_id;
        tokio::spawn(async move {
            registry.deregister(turn_id).await;
        });
    }
}

/// Phase 50.2 Plan 14 (G-50.2-2b) / Phase 50.2 Plan 16 (G-50.2-2c):
/// register-before-spawn around a single subprocess handoff, in `registry`
/// (an owned handle — a clone is another handle to the SAME inner map,
/// which is what lets a caller pass a production or a per-test registry
/// interchangeably). Mints a fresh `TurnId` and `CancellationToken`,
/// registers a `TurnEntry` labelled with [`handoff_turn_session_id`] under
/// `Surface::Cli` (reused deliberately — the child IS the ironhermes CLI
/// binary), and ONLY THEN constructs the `DeregisterOnDrop` guard and moves
/// the owned strings plus a token clone into `tokio::task::spawn_blocking`'s
/// closure, which calls [`run_bot_handoff_with_cancel`]. A `JoinError` (a
/// panicked blocking closure) maps to `BotHandoffError::SpawnFailed`,
/// mirroring the round driver's own mapping. The guard is dropped explicitly
/// before returning, so deregistration is issued on the success path too,
/// not only when the guard's own scope ends. Every registry call here runs
/// in async code — never inside the blocking closure.
///
/// **Plan 16 addition:** `in_flight` is an owned
/// [`crate::server::handoff_steering::InFlightGuard`] the CALLER has already
/// acquired (either via [`crate::server::handoff_steering::mark_bot_in_flight`]
/// for a caller that always runs, or via the `Begin` branch of
/// [`crate::server::handoff_steering::begin_turn_or_queue`] for a caller that
/// may queue instead). It is held for the WHOLE call — bound to a local so
/// it drops only after the blocking closure joins — so the bot reads as busy
/// from before the child is spawned until after it is reaped.
///
/// Phase 50.2 Plan 19 (CR-01): at every production call site this whole
/// call now runs on a task [`await_detached`] spawned, not on the one-shot
/// `#[server]` fn's own request future — so "the WHOLE call" here means the
/// whole call as observed by that detached task, which keeps running (and
/// keeps holding this guard) even after the request that started it has
/// been dropped. Immediately
/// after entering this fn (the guard already exists by construction), this
/// drains the bot's queue and folds it into the prompt via
/// [`crate::server::handoff_steering::fold_steering_into_prompt`] BEFORE the
/// blocking closure is spawned — draining after the guard exists (never
/// before) is what makes a send landing during this window queue for the
/// NEXT turn rather than vanish.
#[cfg(feature = "server")]
pub(crate) async fn run_bot_handoff_tracked_in(
    registry: &ironhermes_core::TurnRegistry,
    in_flight: crate::server::handoff_steering::InFlightGuard,
    bot_name: &str,
    message: &str,
    override_workspace: Option<&str>,
    session_title: Option<&str>,
) -> Result<crate::protocol::BotHandoffResult, BotHandoffError> {
    let turn_id = ironhermes_core::TurnId::new_v4();
    let cancel_token = tokio_util::sync::CancellationToken::new();
    let session_id = handoff_turn_session_id(bot_name, session_title);

    registry
        .register(ironhermes_core::TurnEntry {
            turn_id,
            session_id,
            surface: ironhermes_core::Surface::Cli,
            started_at: std::time::Instant::now(),
            cancel: cancel_token.clone(),
        })
        .await;

    let guard = DeregisterOnDrop {
        registry: registry.clone(),
        turn_id,
    };

    // Plan 16 (G-50.2-2c): drained AFTER the in-flight guard exists, never
    // before — a steering send that lands in the window between this fn
    // being entered and the queue being drained is what MUST fold into
    // THIS turn's prompt rather than being lost. An empty queue folds to a
    // byte-for-byte identical message (`fold_steering_into_prompt`'s own
    // pure no-op contract).
    let queued = crate::server::handoff_steering::drain_steering(bot_name);
    let folded_message = crate::server::handoff_steering::fold_steering_into_prompt(&queued, message);

    let bot_name_owned = bot_name.to_string();
    let message_owned = folded_message;
    let override_workspace_owned = override_workspace.map(|s| s.to_string());
    let session_title_owned = session_title.map(|s| s.to_string());
    let cancel_for_blocking = cancel_token.clone();

    let join_result = tokio::task::spawn_blocking(move || {
        run_bot_handoff_with_cancel(
            &bot_name_owned,
            &message_owned,
            override_workspace_owned.as_deref(),
            session_title_owned.as_deref(),
            Some(&cancel_for_blocking),
        )
    })
    .await;

    // Explicit drop before returning — deregisters on the success path too,
    // not only when the guard's own scope happens to end. `in_flight` drops
    // here too (bound to this fn's own scope, held for the whole call), so
    // the bot only reads as idle again after the child has been reaped.
    drop(guard);
    drop(in_flight);

    match join_result {
        Ok(result) => result,
        Err(e) => Err(BotHandoffError::SpawnFailed {
            reason: e.to_string(),
        }),
    }
}

/// Phase 50.2 Plan 14 (G-50.2-2b) / Phase 50.2 Plan 16 (G-50.2-2c):
/// production wrapper — resolves [`handoff_turn_registry`], unconditionally
/// acquires its own [`crate::server::handoff_steering::InFlightGuard`] via
/// [`crate::server::handoff_steering::mark_bot_in_flight`] (this wrapper
/// always RUNS, never queues), and delegates to
/// [`run_bot_handoff_tracked_in`]. Mirrors this crate's own `run_group_rounds`
/// / `run_group_rounds_with_settings` split: a wrapper that reaches for
/// process state, and an impl that takes it as a parameter so the impl is
/// directly testable with a fresh, per-test registry.
///
/// No production call site — nor any test in this module — reaches this fn
/// any more: `dispatch_bot_message` (Plan 16) routes through
/// [`crate::server::handoff_steering::begin_turn_or_queue`] directly so it
/// can queue instead of always running, and `dispatch_member_turn`
/// (`group_chat_api.rs`) acquires its own guard directly for the same
/// always-runs reason a round member turn must never queue. Kept rather
/// than deleted — it documents the "always run, resolve process state
/// internally" shape `run_bot_handoff_tracked_in`'s two production callers
/// each hand-roll for their own reasons (one needs the write-gate-gated
/// begin-or-queue decision, the other needs an explicit per-test registry
/// parameter) — hence an unconditional `#[allow(dead_code)]` rather than
/// the `#[cfg_attr(not(test), ...)]` form this crate uses for genuinely
/// test-pinned functions.
#[allow(dead_code)]
#[cfg(feature = "server")]
pub(crate) async fn run_bot_handoff_tracked(
    bot_name: &str,
    message: &str,
    override_workspace: Option<&str>,
    session_title: Option<&str>,
) -> Result<crate::protocol::BotHandoffResult, BotHandoffError> {
    let registry = handoff_turn_registry();
    let guard = crate::server::handoff_steering::mark_bot_in_flight(bot_name);
    run_bot_handoff_tracked_in(&registry, guard, bot_name, message, override_workspace, session_title).await
}

/// Phase 50.2 Plan 19 (CR-01): the ONE documented decoupling seam every
/// one-shot `#[server]` dispatch fn (`dispatch_bot_message`,
/// `dispatch_mention_handoff`, `dispatch_group_round`) routes its tracked
/// subprocess dispatch through.
///
/// Each of those fns is a single HTTP request/response round trip, and
/// axum/hyper drops the in-flight handler future for that request the
/// moment the client disconnects — tab close, navigation, or a dropped
/// connection — at any point inside the up-to-`BOT_HANDOFF_TIMEOUT_SECONDS`
/// (180s) window a tracked dispatch can run. If the tracked call were
/// `.await`ed directly inside that request's own future, dropping the
/// future would immediately drop every guard the call owns
/// (`DeregisterOnDrop`, `InFlightGuard`, `RoomDriveGuard`) — while the
/// underlying `tokio::task::spawn_blocking` closure, still holding a live
/// `std::process::Child`, keeps running to completion or timeout entirely
/// detached from any tracking. `tokio::task::spawn_blocking` is, like
/// `tokio::spawn`, NOT cancelled by dropping its `JoinHandle` — that is
/// exactly the mechanism that makes an un-decoupled drop leak a live child.
///
/// The fix is ownership, not transport: `await_detached` spawns `fut` onto
/// its own `tokio::spawn`ed task immediately, so every guard `fut` owns is
/// released only when the subprocess actually finishes or is explicitly
/// cancelled — never merely because the request that started it went away.
/// The caller then awaits only the returned `JoinHandle`'s future; dropping
/// THAT future detaches without cancelling the spawned task, which is
/// precisely the property required.
///
/// `spawn` — not `spawn_local` — is the correct primitive here: this
/// crate's web server runs handlers inside a per-connection `LocalSet`, and
/// a `LocalSet`-bound task dies with the very connection this seam exists
/// to decouple from. `spawn` requires `F: Send + 'static`, which is why
/// every caller of this seam passes owned values into its `async move`
/// block rather than borrows.
#[cfg(feature = "server")]
pub(crate) async fn await_detached<F, T>(fut: F) -> Result<T, tokio::task::JoinError>
where
    F: std::future::Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    tokio::spawn(fut).await
}

/// Phase 50.2 Plan 19 (CR-01): the Bot Chat one-shot dispatch's detached
/// wrapper. Takes OWNED values (not borrows) precisely because the future
/// built from them must be `'static` to be spawned by [`await_detached`].
/// Builds the exact same `run_bot_handoff_tracked_in` call
/// `dispatch_bot_message` used to make inline, wraps it in an `async move`
/// block, and hands that block to `await_detached` so the tracked dispatch
/// now runs on its own task rather than sharing a lifetime with the HTTP
/// request. A `JoinError` (the detached task itself panicking or being
/// aborted) flattens into `BotHandoffError::SpawnFailed`, mirroring
/// `run_bot_handoff_tracked_in`'s own `JoinError` mapping rather than
/// inventing a new error shape.
#[cfg(feature = "server")]
pub(crate) async fn dispatch_bot_message_detached_in(
    registry: ironhermes_core::TurnRegistry,
    in_flight: crate::server::handoff_steering::InFlightGuard,
    bot_name: String,
    message: String,
    session_title: Option<String>,
) -> Result<crate::protocol::BotHandoffResult, BotHandoffError> {
    let fut = async move {
        // `None` for the workspace override is D-06's default, unchanged
        // from the inline call site this wrapper replaces.
        run_bot_handoff_tracked_in(
            &registry,
            in_flight,
            &bot_name,
            &message,
            None,
            session_title.as_deref(),
        )
        .await
    };
    match await_detached(fut).await {
        Ok(result) => result,
        Err(e) => Err(BotHandoffError::SpawnFailed {
            reason: format!("detached bot dispatch task: {e}"),
        }),
    }
}

/// Phase 50.1 Plan 03 (D-04/D-05/D-06/D-13): one-shot send-to-bot dispatch
/// for a non-live roster card's handoff composer. Follows this crate's
/// four-step write protocol: validate (delegated into `run_bot_handoff`) →
/// fresh `Config::load()` → `check_profile_write_gate` (fails closed) →
/// the real work.
///
/// Phase 50.2 Plan 16 (G-50.2-2c): the real work now calls
/// [`crate::server::handoff_steering::begin_turn_or_queue`] — the ONE
/// atomic run-now-or-queue decision — AFTER the write gate (so the queue is
/// never a back door around a closed gate). A busy bot returns
/// `BotDispatchOutcome::Queued` without ever spawning a second concurrent
/// subprocess; an idle bot runs now, holding the returned guard.
///
/// Phase 50.2 Plan 19 (CR-01): the run-now branch no longer `.await`s the
/// tracked call inline — it hands off to
/// [`dispatch_bot_message_detached_in`], which runs the tracked call on its
/// own `tokio::spawn`ed task via [`await_detached`]. This fn's own future
/// (owned by axum/hyper for the lifetime of the HTTP request) can still be
/// dropped on a client disconnect, but the detached task is not — so
/// `DeregisterOnDrop` and `InFlightGuard` release only when the subprocess
/// actually finishes or is explicitly cancelled, never merely because the
/// request went away. Every `BotHandoffError` and `SteeringError` maps to a
/// `ServerFnError` carrying its operator-readable message and nothing more.
#[server]
pub async fn dispatch_bot_message(
    req: crate::protocol::BotHandoffRequest,
) -> Result<crate::protocol::BotDispatchOutcome, ServerFnError> {
    #[cfg(feature = "server")]
    {
        let config = ironhermes_core::config::Config::load()
            .map_err(|e| ServerFnError::new(format!("Config load failed: {e}")))?;
        crate::server::profile_api::check_profile_write_gate(&config)
            .map_err(ServerFnError::new)?;

        let name = req.name;
        let message = req.message;

        match crate::server::handoff_steering::begin_turn_or_queue(&name, &message)
            .map_err(|e| ServerFnError::new(e.to_string()))?
        {
            crate::server::handoff_steering::BeginOrQueue::Queued { depth } => {
                Ok(crate::protocol::BotDispatchOutcome::Queued { depth })
            }
            crate::server::handoff_steering::BeginOrQueue::Begin(guard) => {
                let registry = handoff_turn_registry();
                // Phase 50.2 Plan 02 (D-21): the roster composer's one-shot
                // send is a Bot Chat turn — `Some("Bot Chat")` is where its
                // own conversational continuity across separate dispatches
                // comes from.
                let result = dispatch_bot_message_detached_in(
                    registry,
                    guard,
                    name,
                    message,
                    Some("Bot Chat".to_string()),
                )
                .await
                .map_err(|e| ServerFnError::new(e.to_string()))?;

                Ok(crate::protocol::BotDispatchOutcome::Replied(result))
            }
        }
    }
    #[cfg(not(feature = "server"))]
    {
        let _ = req;
        Err(ServerFnError::new(
            "dispatch_bot_message unavailable without `server` feature",
        ))
    }
}

#[cfg(all(test, feature = "server"))]
mod tests {
    use super::*;

    /// RAII guard that sets/removes an env var and restores the previous
    /// value on drop. Duplicated from `profile_api.rs`'s / `bot_meta_api.rs`'s
    /// own `ScopedEnv` — each `#[cfg(test)]` module is its own namespace, the
    /// crate's own sanctioned "duplicate the guard" precedent, not drift.
    struct ScopedEnv {
        key: String,
        prev: Option<String>,
    }

    impl ScopedEnv {
        fn set(key: &str, value: &str) -> Self {
            let prev = std::env::var(key).ok();
            // SAFETY: single-threaded test context; env_lock() below
            // serializes every test in this crate that mutates process env.
            unsafe { std::env::set_var(key, value) };
            Self {
                key: key.to_string(),
                prev,
            }
        }

        fn remove(key: &str) -> Self {
            let prev = std::env::var(key).ok();
            // SAFETY: see `set` above.
            unsafe { std::env::remove_var(key) };
            Self {
                key: key.to_string(),
                prev,
            }
        }
    }

    impl Drop for ScopedEnv {
        fn drop(&mut self) {
            // SAFETY: still holding env_lock() from the caller.
            match &self.prev {
                Some(v) => unsafe { std::env::set_var(&self.key, v) },
                None => unsafe { std::env::remove_var(&self.key) },
            }
        }
    }

    fn home(dir: &tempfile::TempDir) -> ScopedEnv {
        ScopedEnv::set(
            "IRONHERMES_HOME",
            dir.path().to_str().expect("tempdir path must be utf8"),
        )
    }

    fn scaffold_profile_workspace(name: &str) -> PathBuf {
        let ws = crate::server::profile_api::profile_dir_for(name).join("workspace");
        std::fs::create_dir_all(&ws).expect("mkdir workspace");
        ws
    }

    /// Writes a `#!/bin/sh` stub at `dir/name`, `chmod +x` on unix. Test-only
    /// spawn target, pointed at through `IRONHERMES_WORKER_BIN`.
    fn write_stub_script(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("write stub script");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&path).expect("metadata").permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&path, perms).expect("set_permissions 0755");
        }
        path
    }

    // -------------------------------------------------------------------
    // build_bot_handoff_argv (Task 1)
    // -------------------------------------------------------------------

    #[test]
    fn build_bot_handoff_argv_returns_exact_five_token_vector() {
        let argv = build_bot_handoff_argv("scout", "hello", None);
        assert_eq!(
            argv,
            vec![
                "--profile".to_string(),
                "scout".to_string(),
                "chat".to_string(),
                "-q".to_string(),
                "hello".to_string(),
            ],
            "the None case is the pre-50.2 vector, unchanged"
        );
    }

    /// Phase 50.2 Plan 02 (D-21): `Some(title)` appends exactly two further
    /// argv elements — this is the exact five-plus-two-element vector the
    /// group-chat round driver and the roster composer both spawn.
    #[test]
    fn build_bot_handoff_argv_with_session_title_appends_flag_and_title() {
        let argv = build_bot_handoff_argv("zig", "hi", Some("Group: standup"));
        assert_eq!(
            argv,
            vec![
                "--profile".to_string(),
                "zig".to_string(),
                "chat".to_string(),
                "-q".to_string(),
                "hi".to_string(),
                "--session".to_string(),
                "Group: standup".to_string(),
            ]
        );
    }

    #[test]
    fn build_bot_handoff_argv_hyphen_prefixed_message_still_placed_as_query_value() {
        let argv = build_bot_handoff_argv("scout", "-not-a-flag", None);
        assert_eq!(
            argv,
            vec![
                "--profile".to_string(),
                "scout".to_string(),
                "chat".to_string(),
                "-q".to_string(),
                "-not-a-flag".to_string(),
            ],
            "a hyphen-prefixed message must still be the query flag's value and add no tokens"
        );
    }

    // -------------------------------------------------------------------
    // build_bot_handoff_env (Task 1)
    // -------------------------------------------------------------------

    #[test]
    fn build_bot_handoff_env_omits_a_probe_variable_not_on_the_allowlist() {
        let _lock = crate::server::test_support::env_lock();
        let _probe = ScopedEnv::set("BOT_HANDOFF_TEST_PROBE_VAR", "leak-test-value");

        let env = build_bot_handoff_env();

        assert!(
            !env.contains_key("BOT_HANDOFF_TEST_PROBE_VAR"),
            "a non-allowlisted variable must never appear in the child env"
        );
    }

    #[test]
    fn build_bot_handoff_env_copies_allowlisted_var_verbatim_when_present() {
        let _lock = crate::server::test_support::env_lock();
        let _term = ScopedEnv::set("TERM", "bot-handoff-test-term");

        let env = build_bot_handoff_env();

        assert_eq!(
            env.get("TERM").map(String::as_str),
            Some("bot-handoff-test-term")
        );
    }

    #[test]
    fn build_bot_handoff_env_omits_allowlisted_var_when_absent() {
        let _lock = crate::server::test_support::env_lock();
        let _term = ScopedEnv::remove("TERM");

        let env = build_bot_handoff_env();

        assert!(
            !env.contains_key("TERM"),
            "an allowlisted variable absent from the parent env must be omitted, not empty-valued"
        );
    }

    #[test]
    fn build_bot_handoff_env_forces_rust_log_off_even_when_parent_has_rust_log_set() {
        // OF-5 (gap task): RUST_LOG is orchestrator-controlled for this
        // headless dispatch path — the operator's own shell RUST_LOG must
        // never leak through and re-enable the log-vs-reply interleaving
        // bug this override fixes.
        let _lock = crate::server::test_support::env_lock();
        let _rust_log = ScopedEnv::set("RUST_LOG", "debug");

        let env = build_bot_handoff_env();

        assert_eq!(env.get("RUST_LOG").map(String::as_str), Some("off"));
    }

    #[test]
    fn build_bot_handoff_env_sets_rust_log_off_when_parent_has_no_rust_log_at_all() {
        let _lock = crate::server::test_support::env_lock();
        let _rust_log = ScopedEnv::remove("RUST_LOG");

        let env = build_bot_handoff_env();

        assert_eq!(env.get("RUST_LOG").map(String::as_str), Some("off"));
    }

    #[test]
    fn build_bot_handoff_env_always_sets_ironhermes_home_from_resolved_hermes_home() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);

        let env = build_bot_handoff_env();

        assert_eq!(
            env.get("IRONHERMES_HOME").map(String::as_str),
            dir.path().to_str(),
            "IRONHERMES_HOME must always be pinned to the resolved hermes home"
        );
    }

    // -------------------------------------------------------------------
    // resolve_bot_workspace_dir (Task 1)
    // -------------------------------------------------------------------

    #[test]
    fn resolve_bot_workspace_dir_rejects_invalid_profile_name() {
        let err = resolve_bot_workspace_dir("Not Valid!", None).unwrap_err();
        assert!(matches!(err, BotHandoffError::InvalidProfileName { .. }));
    }

    #[test]
    fn resolve_bot_workspace_dir_rejects_empty_override() {
        let err = resolve_bot_workspace_dir("scout", Some("")).unwrap_err();
        assert!(matches!(err, BotHandoffError::WorkspacePathRejected { .. }));
    }

    #[test]
    fn resolve_bot_workspace_dir_rejects_dollar_sign_override() {
        let err =
            resolve_bot_workspace_dir("scout", Some("$IRONHERMES_HOME/profiles/scout/workspace"))
                .unwrap_err();
        match err {
            BotHandoffError::WorkspacePathRejected { reason, .. } => {
                assert!(reason.contains('$') || reason.to_lowercase().contains("unexpanded"));
            }
            other => panic!("expected WorkspacePathRejected, got {other:?}"),
        }
    }

    #[test]
    fn resolve_bot_workspace_dir_rejects_relative_override() {
        let err = resolve_bot_workspace_dir("scout", Some("relative/path")).unwrap_err();
        assert!(matches!(err, BotHandoffError::WorkspacePathRejected { .. }));
    }

    #[test]
    fn resolve_bot_workspace_dir_rejects_missing_absolute_override() {
        let dir = tempfile::tempdir().expect("tempdir");
        let missing = dir.path().join("does-not-exist-subdir");
        assert!(!missing.exists());
        let err = resolve_bot_workspace_dir("scout", missing.to_str()).unwrap_err();
        assert!(matches!(err, BotHandoffError::WorkspaceNotFound { .. }));
    }

    #[test]
    fn resolve_bot_workspace_dir_accepts_real_absolute_override() {
        let dir = tempfile::tempdir().expect("tempdir");
        let resolved = resolve_bot_workspace_dir("scout", dir.path().to_str())
            .expect("existing absolute dir should resolve Ok");
        assert!(resolved.is_absolute());
        assert!(resolved.is_dir());
    }

    #[test]
    fn resolve_bot_workspace_dir_defaults_to_profile_workspace_subdir() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        scaffold_profile_workspace("scout");

        let resolved =
            resolve_bot_workspace_dir("scout", None).expect("default workspace should resolve");
        assert!(resolved.ends_with("profiles/scout/workspace"));
    }

    // -------------------------------------------------------------------
    // resolve_bot_worker_bin (Task 1)
    // -------------------------------------------------------------------

    #[test]
    fn resolve_bot_worker_bin_falls_back_to_ironhermes_when_unset() {
        let _lock = crate::server::test_support::env_lock();
        let _guard = ScopedEnv::remove("IRONHERMES_WORKER_BIN");
        assert_eq!(resolve_bot_worker_bin(), Ok("ironhermes".to_string()));
    }

    #[test]
    fn resolve_bot_worker_bin_rejects_blank_override() {
        let _lock = crate::server::test_support::env_lock();
        let _guard = ScopedEnv::set("IRONHERMES_WORKER_BIN", "   ");
        assert_eq!(
            resolve_bot_worker_bin(),
            Err(BotHandoffError::WorkerBinaryNotResolvable)
        );
    }

    // -------------------------------------------------------------------
    // run_bot_handoff (Task 2)
    // -------------------------------------------------------------------

    #[test]
    fn run_bot_handoff_for_the_live_profile_returns_streaming_error_without_spawning() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        // A home path shaped like `<root>/profiles/scout` makes
        // `ironhermes_core::current_profile()` resolve to "scout" (it
        // reverse-walks `get_hermes_home()` for a `profiles/<slug>`
        // ancestor) — no stub binary needed, the guard must fire first.
        let scoped_home = dir.path().join("profiles").join("scout");
        std::fs::create_dir_all(&scoped_home).expect("mkdir scoped home");
        let _guard = ScopedEnv::set(
            "IRONHERMES_HOME",
            scoped_home.to_str().expect("utf8 path"),
        );

        let result = run_bot_handoff("scout", "hi", None, None);
        assert_eq!(result, Err(BotHandoffError::LiveProfileMustStream));
    }

    #[test]
    fn run_bot_handoff_stub_binary_nonzero_exit_returns_nonzero_variant_and_writes_no_preview() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        scaffold_profile_workspace("scout");
        let stub = write_stub_script(dir.path(), "stub-fail.sh", "exit 7");
        let _bin_guard = ScopedEnv::set(
            "IRONHERMES_WORKER_BIN",
            stub.to_str().expect("utf8 stub path"),
        );

        let result = run_bot_handoff("scout", "hello", None, None);
        assert_eq!(result, Err(BotHandoffError::NonZeroExit { code: 7 }));

        let index = load_bot_meta_map(&crate::server::bot_meta_api::bot_meta_index_path())
            .expect("load index");
        assert!(
            !index.contains_key("scout"),
            "a failed dispatch must never write a preview"
        );
    }

    #[test]
    fn run_bot_handoff_stub_binary_success_returns_reply_writes_preview_and_positive_elapsed() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        scaffold_profile_workspace("scout");
        let stub = write_stub_script(
            dir.path(),
            "stub-ok.sh",
            "sleep 0.02\necho 'hello from scout'",
        );
        let _bin_guard = ScopedEnv::set(
            "IRONHERMES_WORKER_BIN",
            stub.to_str().expect("utf8 stub path"),
        );

        let result = run_bot_handoff("scout", "hello", None, None).expect("stub success should Ok");
        assert_eq!(result.reply, "hello from scout");
        assert!(result.exit_ok);
        assert!(result.error.is_none());
        assert!(result.elapsed_ms > 0, "elapsed_ms must be > 0, got 0");

        let index = load_bot_meta_map(&crate::server::bot_meta_api::bot_meta_index_path())
            .expect("load index");
        let meta = index.get("scout").expect("preview must be written for scout");
        assert_eq!(meta.preview.as_deref(), Some("hello from scout"));
        assert!(meta.preview_at_ms.is_some());
    }

    #[test]
    fn run_bot_handoff_failed_dispatch_never_overwrites_an_existing_preview() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        scaffold_profile_workspace("scout");

        // Seed an existing preview via the real write-back path used on
        // success, so this test proves the SAME mechanism, not a hand-built
        // fixture.
        crate::server::bot_meta_api::write_preview_for("scout", "earlier reply", 1_700_000_000_000)
            .expect("seed preview");

        let stub = write_stub_script(dir.path(), "stub-fail2.sh", "exit 1");
        let _bin_guard = ScopedEnv::set(
            "IRONHERMES_WORKER_BIN",
            stub.to_str().expect("utf8 stub path"),
        );

        let result = run_bot_handoff("scout", "hello", None, None);
        assert!(result.is_err());

        let index = load_bot_meta_map(&crate::server::bot_meta_api::bot_meta_index_path())
            .expect("load index");
        let meta = index.get("scout").expect("seeded preview must still exist");
        assert_eq!(
            meta.preview.as_deref(),
            Some("earlier reply"),
            "a failed dispatch must never clobber an existing preview"
        );
    }

    // -------------------------------------------------------------------
    // OF-5 (gap task): strip_ansi_escapes / looks_like_tracing_log_line /
    // sanitize_reply — pure logic, no process spawn needed
    // -------------------------------------------------------------------

    #[test]
    fn strip_ansi_escapes_removes_sgr_color_codes() {
        let raw = "\u{1b}[2m2026-08-19T01:04:10.265236Z\u{1b}[0m \u{1b}[32m INFO\u{1b}[0m hello";
        let result = strip_ansi_escapes(raw);
        assert_eq!(
            result,
            "2026-08-19T01:04:10.265236Z  INFO hello",
            "every CSI/SGR escape must be removed, plain text preserved verbatim"
        );
        assert!(!result.contains('\u{1b}'), "no raw ESC byte may remain");
    }

    #[test]
    fn strip_ansi_escapes_is_a_no_op_on_plain_text() {
        let raw = "just a normal reply, no escapes here";
        assert_eq!(strip_ansi_escapes(raw), raw);
    }

    #[test]
    fn strip_ansi_escapes_drops_bare_esc_byte_without_eating_the_next_char() {
        let raw = "a\u{1b}b";
        assert_eq!(strip_ansi_escapes(raw), "ab");
    }

    #[test]
    fn looks_like_tracing_log_line_detects_iso8601_prefixed_line() {
        assert!(looks_like_tracing_log_line(
            "2026-08-19T01:04:10.265236Z  INFO some tracing message"
        ));
    }

    #[test]
    fn looks_like_tracing_log_line_tolerates_leading_whitespace() {
        assert!(looks_like_tracing_log_line(
            "  2026-08-19T01:04:10.265236Z  WARN indented tracing line"
        ));
    }

    #[test]
    fn looks_like_tracing_log_line_false_for_ordinary_reply_text() {
        assert!(!looks_like_tracing_log_line("Sure, I can help with that."));
        assert!(!looks_like_tracing_log_line(
            "1. First item in a numbered list"
        ));
    }

    #[test]
    fn looks_like_tracing_log_line_false_for_short_line() {
        assert!(!looks_like_tracing_log_line("2026-08"));
    }

    #[test]
    fn sanitize_reply_strips_interleaved_ansi_log_noise_and_keeps_the_real_reply() {
        // Synthetic child output: an ANSI-escaped tracing line, then the
        // bot's real reply — the exact interleaving shape OF-5's evidence
        // screenshot showed leaking into a roster card's preview.
        let raw = b"\x1b[2m2026-08-19T01:04:10.265236Z\x1b[0m \x1b[32m INFO\x1b[0m ironhermes: turn started\nHello! Here is the answer you asked for.\n";
        let reply = sanitize_reply(raw).expect("real reply text must sanitize to Ok");
        assert_eq!(reply, "Hello! Here is the answer you asked for.");
    }

    #[test]
    fn sanitize_reply_multiple_log_lines_around_a_multiline_reply() {
        let raw = b"\x1b[2m2026-08-19T01:04:10.000000Z\x1b[0m \x1b[32m INFO\x1b[0m starting turn\nLine one of the reply.\nLine two of the reply.\n\x1b[2m2026-08-19T01:04:11.000000Z\x1b[0m \x1b[33m WARN\x1b[0m context trimmed\n";
        let reply = sanitize_reply(raw).expect("must sanitize to Ok");
        assert_eq!(reply, "Line one of the reply.\nLine two of the reply.");
    }

    #[test]
    fn sanitize_reply_output_that_is_only_log_lines_returns_named_error_never_log_text() {
        let raw = b"\x1b[2m2026-08-19T01:04:10.265236Z\x1b[0m \x1b[32m INFO\x1b[0m turn started\n\x1b[2m2026-08-19T01:04:11.000000Z\x1b[0m \x1b[32m INFO\x1b[0m turn finished\n";
        let err = sanitize_reply(raw).expect_err("log-only output must never be treated as a reply");
        assert!(matches!(err, BotHandoffError::ReplyWasLogOnly { .. }));
    }

    #[test]
    fn sanitize_reply_genuinely_empty_raw_output_stays_a_legitimate_empty_reply() {
        // Distinguishes "child printed nothing at all" (legitimate, unchanged
        // pre-OF-5 behavior) from "child printed content that was entirely
        // log noise" (the new named-error path above).
        let reply = sanitize_reply(b"").expect("empty raw output must still be Ok");
        assert_eq!(reply, "");
    }

    #[test]
    fn sanitize_reply_plain_reply_with_no_log_lines_passes_through_trimmed() {
        let raw = b"  Hello there!  \n";
        let reply = sanitize_reply(raw).expect("must sanitize to Ok");
        assert_eq!(reply, "Hello there!");
    }

    // -------------------------------------------------------------------
    // OF-9 (gap task): strip_think_blocks — pure logic
    // -------------------------------------------------------------------

    #[test]
    fn strip_think_blocks_removes_a_single_closed_block() {
        let raw = "<think>reasoning about the answer</think>Here is the answer.";
        assert_eq!(strip_think_blocks(raw), "Here is the answer.");
    }

    #[test]
    fn strip_think_blocks_removes_a_multiline_closed_block() {
        let raw = "<think>\nline one of reasoning\nline two of reasoning\n</think>\nThe real reply.";
        assert_eq!(strip_think_blocks(raw), "\nThe real reply.");
    }

    #[test]
    fn strip_think_blocks_removes_multiple_blocks() {
        let raw = "<think>first</think>Part one. <think>second</think>Part two.";
        assert_eq!(strip_think_blocks(raw), "Part one. Part two.");
    }

    #[test]
    fn strip_think_blocks_unclosed_leading_tag_strips_to_end() {
        // The exact evidenced shape: the reasoning block OPENS the reply and
        // never closes (e.g. a truncated/cut-off model output) — strips to
        // the end of the string rather than leaving a dangling raw tag.
        let raw = "<think>reasoning that never closes because output got cut off";
        assert_eq!(strip_think_blocks(raw), "");
    }

    #[test]
    fn strip_think_blocks_unclosed_tag_after_real_content_keeps_the_real_content() {
        let raw = "Real answer first. <think>trailing unclosed reasoning";
        assert_eq!(strip_think_blocks(raw), "Real answer first. ");
    }

    #[test]
    fn strip_think_blocks_think_only_reply_strips_to_empty() {
        let raw = "<think>only reasoning, no real reply at all</think>";
        assert_eq!(strip_think_blocks(raw), "");
    }

    #[test]
    fn strip_think_blocks_no_op_when_no_think_tag_present() {
        let raw = "A perfectly ordinary reply with no think tags.";
        assert_eq!(strip_think_blocks(raw), raw);
    }

    #[test]
    fn strip_think_blocks_no_op_on_empty_string() {
        assert_eq!(strip_think_blocks(""), "");
    }

    // -------------------------------------------------------------------
    // OF-5 (gap task): run_bot_handoff end-to-end through a stub subprocess
    // -------------------------------------------------------------------

    #[test]
    fn run_bot_handoff_strips_ansi_log_noise_from_stub_stdout_and_writes_clean_preview() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        scaffold_profile_workspace("scout");
        // `printf` emits real ESC bytes for the ANSI portion, followed by
        // the "reply" on its own trailing echo — mirrors a real
        // `tracing_subscriber::fmt()` INFO line landing on stdout ahead of
        // the streamed reply text.
        let stub = write_stub_script(
            dir.path(),
            "stub-ansi.sh",
            "printf '\\033[2m2026-08-19T01:04:10.265236Z\\033[0m \\033[32m INFO\\033[0m turn started\\n'\necho 'the real answer'",
        );
        let _bin_guard = ScopedEnv::set(
            "IRONHERMES_WORKER_BIN",
            stub.to_str().expect("utf8 stub path"),
        );

        let result = run_bot_handoff("scout", "hello", None, None).expect("stub success should Ok");
        assert_eq!(result.reply, "the real answer");
        assert!(
            !result.reply.contains('\u{1b}'),
            "reply must never contain a raw ANSI escape byte"
        );

        let index = load_bot_meta_map(&crate::server::bot_meta_api::bot_meta_index_path())
            .expect("load index");
        let meta = index.get("scout").expect("preview must be written for scout");
        assert_eq!(meta.preview.as_deref(), Some("the real answer"));
    }

    #[test]
    fn run_bot_handoff_stub_emitting_only_log_noise_errors_and_writes_no_preview() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        scaffold_profile_workspace("scout");
        let stub = write_stub_script(
            dir.path(),
            "stub-log-only.sh",
            "printf '\\033[2m2026-08-19T01:04:10.265236Z\\033[0m \\033[32m INFO\\033[0m turn started\\n'\nprintf '\\033[2m2026-08-19T01:04:11.000000Z\\033[0m \\033[32m INFO\\033[0m turn finished\\n'",
        );
        let _bin_guard = ScopedEnv::set(
            "IRONHERMES_WORKER_BIN",
            stub.to_str().expect("utf8 stub path"),
        );

        let result = run_bot_handoff("scout", "hello", None, None);
        assert!(matches!(
            result,
            Err(BotHandoffError::ReplyWasLogOnly { .. })
        ));

        let index = load_bot_meta_map(&crate::server::bot_meta_api::bot_meta_index_path())
            .expect("load index");
        assert!(
            !index.contains_key("scout"),
            "a log-only reply must never be written back as a preview"
        );
    }

    // -------------------------------------------------------------------
    // OF-6 (gap task): run_bot_handoff self-heals a pre-fix legacy persona
    // -------------------------------------------------------------------

    #[test]
    fn run_bot_handoff_migrates_a_legacy_persona_before_dispatch() {
        // Proves the zig-shaped scenario end-to-end through the REAL
        // dispatch entry point: a persona saved before the OF-6 fix (at the
        // legacy `workspace/SOUL.md` location) is migrated to the
        // canonical profile-root SOUL.md before the subprocess is spawned
        // — so the very next turn loads it, with no operator action.
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        let workspace = scaffold_profile_workspace("scout");
        std::fs::write(workspace.join("SOUL.md"), "Legacy persona, pre-OF-6")
            .expect("seed legacy persona");
        let stub = write_stub_script(dir.path(), "stub-migrate.sh", "echo 'hi'");
        let _bin_guard = ScopedEnv::set(
            "IRONHERMES_WORKER_BIN",
            stub.to_str().expect("utf8 stub path"),
        );

        run_bot_handoff("scout", "hello", None, None).expect("stub success should Ok");

        assert_eq!(
            std::fs::read_to_string(
                crate::server::profile_api::profile_dir_for("scout").join("SOUL.md")
            )
            .expect("canonical persona must exist after dispatch"),
            "Legacy persona, pre-OF-6"
        );
        assert!(
            !workspace.join("SOUL.md").exists(),
            "the legacy location must be cleaned up once migrated"
        );
    }

    // -------------------------------------------------------------------
    // OF-9 (gap task): run_bot_handoff end-to-end — <think> block handling
    // -------------------------------------------------------------------

    #[test]
    fn run_bot_handoff_think_only_reply_succeeds_and_writes_no_preview() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        scaffold_profile_workspace("scout");
        let stub = write_stub_script(
            dir.path(),
            "stub-think-only.sh",
            "echo '<think>only reasoning, nothing else</think>'",
        );
        let _bin_guard = ScopedEnv::set(
            "IRONHERMES_WORKER_BIN",
            stub.to_str().expect("utf8 stub path"),
        );

        let result = run_bot_handoff("scout", "hello", None, None)
            .expect("a think-only reply must still be a successful dispatch, not an error");

        assert!(
            result.reply.contains("<think>"),
            "the reply delivered to the caller must keep the full text (think tags intact) — \
             the chat window is responsible for its own render-time hiding"
        );
        assert!(result.exit_ok);

        let index = load_bot_meta_map(&crate::server::bot_meta_api::bot_meta_index_path())
            .expect("load index");
        assert!(
            !index.contains_key("scout"),
            "a think-only reply must never write an empty-string preview"
        );
    }

    #[test]
    fn run_bot_handoff_think_only_reply_does_not_clobber_an_existing_preview() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        scaffold_profile_workspace("scout");

        // First dispatch: a real reply, establishes a preview.
        let stub_real = write_stub_script(dir.path(), "stub-real.sh", "echo 'the first real answer'");
        let _bin_guard_first = ScopedEnv::set(
            "IRONHERMES_WORKER_BIN",
            stub_real.to_str().expect("utf8 stub path"),
        );
        run_bot_handoff("scout", "hello", None, None).expect("first dispatch should Ok");
        let index = load_bot_meta_map(&crate::server::bot_meta_api::bot_meta_index_path())
            .expect("load index");
        assert_eq!(
            index
                .get("scout")
                .expect("preview must exist after first dispatch")
                .preview
                .as_deref(),
            Some("the first real answer")
        );

        // Second dispatch: think-only — must not blank out the existing preview.
        let stub_think = write_stub_script(
            dir.path(),
            "stub-think.sh",
            "echo '<think>just thinking, no answer this time</think>'",
        );
        let _bin_guard_second = ScopedEnv::set(
            "IRONHERMES_WORKER_BIN",
            stub_think.to_str().expect("utf8 stub path"),
        );
        run_bot_handoff("scout", "hello again", None, None)
            .expect("a think-only reply must still be a successful dispatch");

        let index_after = load_bot_meta_map(&crate::server::bot_meta_api::bot_meta_index_path())
            .expect("load index");
        assert_eq!(
            index_after
                .get("scout")
                .expect("preview must still exist")
                .preview
                .as_deref(),
            Some("the first real answer"),
            "a think-only reply must never clobber an existing preview"
        );
    }

    #[test]
    fn run_bot_handoff_think_block_plus_real_content_writes_preview_without_think_text() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        scaffold_profile_workspace("scout");
        let stub = write_stub_script(
            dir.path(),
            "stub-think-plus-real.sh",
            "echo '<think>internal reasoning nobody should see</think>The actual answer.'",
        );
        let _bin_guard = ScopedEnv::set(
            "IRONHERMES_WORKER_BIN",
            stub.to_str().expect("utf8 stub path"),
        );

        let result = run_bot_handoff("scout", "hello", None, None).expect("stub success should Ok");

        assert!(
            result.reply.contains("<think>"),
            "the delivered reply keeps the full text, think tags intact"
        );
        assert!(result.reply.contains("The actual answer."));

        let index = load_bot_meta_map(&crate::server::bot_meta_api::bot_meta_index_path())
            .expect("load index");
        let meta = index.get("scout").expect("preview must be written for scout");
        let preview = meta.preview.as_deref().expect("preview must be Some");
        assert!(
            !preview.contains("<think>") && !preview.contains("internal reasoning"),
            "the D-13 preview must never contain think-block content"
        );
        assert!(preview.contains("The actual answer."));
    }

    #[test]
    fn run_bot_handoff_plain_reply_with_no_think_tags_preview_matches_reply_unchanged() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        scaffold_profile_workspace("scout");
        let stub = write_stub_script(dir.path(), "stub-plain.sh", "echo 'a perfectly ordinary reply'");
        let _bin_guard = ScopedEnv::set(
            "IRONHERMES_WORKER_BIN",
            stub.to_str().expect("utf8 stub path"),
        );

        let result = run_bot_handoff("scout", "hello", None, None).expect("stub success should Ok");
        assert_eq!(result.reply, "a perfectly ordinary reply");

        let index = load_bot_meta_map(&crate::server::bot_meta_api::bot_meta_index_path())
            .expect("load index");
        let meta = index.get("scout").expect("preview must be written for scout");
        assert_eq!(meta.preview.as_deref(), Some("a perfectly ordinary reply"));
    }

    // -------------------------------------------------------------------
    // stderr_indicates_missing_key (Task 2, pure logic)
    // -------------------------------------------------------------------

    #[test]
    fn stderr_indicates_missing_key_detects_401_unauthorized() {
        assert!(stderr_indicates_missing_key(
            b"error: request failed: 401 Unauthorized"
        ));
    }

    #[test]
    fn stderr_indicates_missing_key_false_for_unrelated_failure() {
        assert!(!stderr_indicates_missing_key(b"panic: index out of bounds"));
    }

    #[test]
    fn stderr_indicates_missing_key_still_true_for_bare_unauthorized() {
        // "unauthorized" alone (no adjacent digit) must still classify —
        // MI-02 only tightened the standalone "401" marker.
        assert!(stderr_indicates_missing_key(
            b"error: request rejected: unauthorized"
        ));
    }

    #[test]
    fn stderr_indicates_missing_key_mi02_false_for_unrelated_401_substrings() {
        // MI-02 regression (`50.1-REVIEW.md`): a bare "401" used to match
        // anywhere in stderr, misclassifying unrelated failures whose text
        // happens to contain "4010", a line number like ":401:", or a
        // port/id containing 401 as a missing-key error.
        assert!(!stderr_indicates_missing_key(
            b"panic: thread panicked at src/main.rs:401:5"
        ));
        assert!(!stderr_indicates_missing_key(
            b"connection refused: 127.0.0.1:4010"
        ));
        assert!(!stderr_indicates_missing_key(
            b"worker 401234 exited with a fatal error"
        ));
    }

    // Re-exported for the `load_bot_meta_map` call above — the fn itself is
    // `pub(crate)` in `bot_meta_api.rs`, imported here rather than
    // re-implemented.
    use crate::server::bot_meta_api::load_bot_meta_map;

    // -------------------------------------------------------------------
    // handoff_turn_session_id (Phase 50.2 Plan 14, Task 1)
    // -------------------------------------------------------------------

    #[test]
    fn handoff_turn_session_id_with_title_renders_name_dot_title() {
        assert_eq!(
            handoff_turn_session_id("zig", Some("Group: standup")),
            "zig \u{b7} Group: standup"
        );
    }

    #[test]
    fn handoff_turn_session_id_without_title_renders_bare_bot_name() {
        assert_eq!(handoff_turn_session_id("zig", None), "zig");
    }

    #[test]
    fn handoff_turn_session_id_whitespace_only_title_treated_as_absent() {
        assert_eq!(handoff_turn_session_id("zig", Some("   ")), "zig");
        assert_eq!(handoff_turn_session_id("zig", Some("")), "zig");
    }

    // -------------------------------------------------------------------
    // spawn_and_capture cancellation (Phase 50.2 Plan 14, Task 1)
    // -------------------------------------------------------------------

    #[allow(clippy::await_holding_lock)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn spawn_and_capture_observes_cancellation_and_kills_the_child() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        scaffold_profile_workspace("zig");
        let stub = write_stub_script(
            dir.path(),
            "stub-sleep30.sh",
            "sleep 30\necho 'should never be seen'",
        );
        let _bin_guard = ScopedEnv::set(
            "IRONHERMES_WORKER_BIN",
            stub.to_str().expect("utf8 stub path"),
        );

        let token = tokio_util::sync::CancellationToken::new();
        let token_for_task = token.clone();
        let start = std::time::Instant::now();
        let handle = tokio::task::spawn_blocking(move || {
            run_bot_handoff_with_cancel("zig", "hello", None, None, Some(&token_for_task))
        });
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        token.cancel();
        let result = handle.await.expect("spawn_blocking join must succeed");
        assert_eq!(result, Err(BotHandoffError::Cancelled));
        assert!(
            start.elapsed() < std::time::Duration::from_secs(10),
            "cancellation must short-circuit well before the 180s ceiling, elapsed={:?}",
            start.elapsed()
        );
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn run_bot_handoff_with_cancel_none_matches_run_bot_handoff() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        scaffold_profile_workspace("zig");
        let stub = write_stub_script(dir.path(), "stub-fixed-reply.sh", "echo 'fixed reply text'");
        let _bin_guard = ScopedEnv::set(
            "IRONHERMES_WORKER_BIN",
            stub.to_str().expect("utf8 stub path"),
        );

        let four_arg = tokio::task::spawn_blocking(|| run_bot_handoff("zig", "hello", None, None))
            .await
            .expect("spawn_blocking join must succeed")
            .expect("four-argument form should Ok");
        let five_arg = tokio::task::spawn_blocking(|| {
            run_bot_handoff_with_cancel("zig", "hello", None, None, None)
        })
        .await
        .expect("spawn_blocking join must succeed")
        .expect("five-argument form should Ok");

        assert_eq!(four_arg.reply, five_arg.reply);
        assert_eq!(four_arg.reply, "fixed reply text");
    }

    // -------------------------------------------------------------------
    // run_bot_handoff_tracked_in (Phase 50.2 Plan 14, Task 2)
    // -------------------------------------------------------------------

    #[allow(clippy::await_holding_lock)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn tracked_handoff_is_listed_while_the_child_runs_and_gone_after() {
        let _lock = crate::server::test_support::env_lock();
        crate::server::handoff_steering::reset_steering_for_test();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        scaffold_profile_workspace("zig");
        let stub = write_stub_script(
            dir.path(),
            "stub-tracked-ok.sh",
            "sleep 2\necho 'tracked reply'",
        );
        let _bin_guard = ScopedEnv::set(
            "IRONHERMES_WORKER_BIN",
            stub.to_str().expect("utf8 stub path"),
        );

        let registry = ironhermes_core::TurnRegistry::new();
        let registry_for_poll = registry.clone();
        let in_flight = crate::server::handoff_steering::mark_bot_in_flight("zig");
        let task = tokio::spawn(async move {
            run_bot_handoff_tracked_in(&registry, in_flight, "zig", "hello", None, Some("Bot Chat"))
                .await
        });

        // Poll list_all() on a short interval for up to 5s until non-empty.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut observed = Vec::new();
        while std::time::Instant::now() < deadline {
            observed = registry_for_poll.list_all().await;
            if !observed.is_empty() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert_eq!(
            observed.len(),
            1,
            "exactly one summary must be listed while the tracked handoff's child runs"
        );
        assert_eq!(
            observed[0].session_id,
            handoff_turn_session_id("zig", Some("Bot Chat"))
        );
        assert_eq!(
            observed[0].surface.to_string(),
            ironhermes_core::Surface::Cli.to_string()
        );

        let result = task.await.expect("join must succeed");
        assert_eq!(result.expect("stub success should Ok").reply, "tracked reply");

        // Bounded poll — the guard's deregister is detached (Drop is
        // synchronous, deregister is async), so it may land a beat after
        // the awaited call returns.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if registry_for_poll.list_all().await.is_empty() {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the registry must become empty within the bounded poll window once the tracked handoff resolves"
            );
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn tracked_handoff_cancel_one_terminates_the_turn_and_clears_the_registry() {
        let _lock = crate::server::test_support::env_lock();
        crate::server::handoff_steering::reset_steering_for_test();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        scaffold_profile_workspace("zig");
        let stub = write_stub_script(
            dir.path(),
            "stub-tracked-cancel.sh",
            "sleep 30\necho 'should never be seen'",
        );
        let _bin_guard = ScopedEnv::set(
            "IRONHERMES_WORKER_BIN",
            stub.to_str().expect("utf8 stub path"),
        );

        let registry = ironhermes_core::TurnRegistry::new();
        let registry_for_poll = registry.clone();
        let start = std::time::Instant::now();
        let in_flight = crate::server::handoff_steering::mark_bot_in_flight("zig");
        let task = tokio::spawn(async move {
            run_bot_handoff_tracked_in(&registry, in_flight, "zig", "hello", None, Some("Bot Chat"))
                .await
        });

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut turn_id = None;
        while std::time::Instant::now() < deadline {
            let observed = registry_for_poll.list_all().await;
            if let Some(summary) = observed.first() {
                turn_id = Some(summary.turn_id);
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        let turn_id = turn_id.expect("a turn must be listed before it can be cancelled");

        let cancelled = registry_for_poll.cancel_one(turn_id).await;
        assert!(cancelled, "cancel_one must find the listed turn");

        let result = task.await.expect("join must succeed");
        assert_eq!(result, Err(BotHandoffError::Cancelled));
        assert!(
            start.elapsed() < std::time::Duration::from_secs(10),
            "cancellation must complete well under ten seconds, elapsed={:?}",
            start.elapsed()
        );

        // Bounded poll for the empty assertion — the guard's deregister is
        // detached, so it may land a beat after the awaited call returns.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if registry_for_poll.list_all().await.is_empty() {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "registry must become empty within the bounded poll window"
            );
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn tracked_handoff_failing_child_still_deregisters() {
        let _lock = crate::server::test_support::env_lock();
        crate::server::handoff_steering::reset_steering_for_test();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        scaffold_profile_workspace("zig");
        let stub = write_stub_script(dir.path(), "stub-tracked-fail.sh", "exit 9");
        let _bin_guard = ScopedEnv::set(
            "IRONHERMES_WORKER_BIN",
            stub.to_str().expect("utf8 stub path"),
        );

        let registry = ironhermes_core::TurnRegistry::new();
        let in_flight = crate::server::handoff_steering::mark_bot_in_flight("zig");
        let result = run_bot_handoff_tracked_in(
            &registry,
            in_flight,
            "zig",
            "hello",
            None,
            Some("Bot Chat"),
        )
        .await;
        assert!(result.is_err(), "a failing child must yield Err");

        // Bounded poll — the guard's deregister is detached (Drop is
        // synchronous, deregister is async), so it may land a beat after
        // the awaited call returns.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if registry.list_all().await.is_empty() {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the registry must become empty within the bounded poll window after a failing tracked handoff"
            );
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }

    // -------------------------------------------------------------------
    // await_detached / dispatch_bot_message_detached_in (Phase 50.2 Plan 19, CR-01)
    // -------------------------------------------------------------------

    #[allow(clippy::await_holding_lock)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn dropping_the_bot_dispatch_future_keeps_the_turn_registered_and_the_bot_busy() {
        let _lock = crate::server::test_support::env_lock();
        crate::server::handoff_steering::reset_steering_for_test();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        scaffold_profile_workspace("zig");
        let stub = write_stub_script(
            dir.path(),
            "stub-detached-drop.sh",
            "sleep 5\necho 'should not be observed by the dropped future'",
        );
        let _bin_guard = ScopedEnv::set(
            "IRONHERMES_WORKER_BIN",
            stub.to_str().expect("utf8 stub path"),
        );

        let registry = ironhermes_core::TurnRegistry::new();
        let registry_for_poll = registry.clone();
        let in_flight = crate::server::handoff_steering::mark_bot_in_flight("zig");

        // Simulated client disconnect: this caller's own future (standing
        // in for the HTTP request future axum/hyper would drop) is wrapped
        // in a short timeout and DROPPED when it elapses, well before the
        // stub's 5s sleep finishes.
        let dropped = tokio::time::timeout(
            std::time::Duration::from_millis(300),
            dispatch_bot_message_detached_in(
                registry,
                in_flight,
                "zig".to_string(),
                "hello".to_string(),
                Some("Bot Chat".to_string()),
            ),
        )
        .await;
        assert!(
            dropped.is_err(),
            "the timeout must elapse — the stub's 5s sleep must still be running"
        );

        tokio::time::sleep(std::time::Duration::from_secs(1)).await;

        let observed = registry_for_poll.list_all().await;
        assert_eq!(
            observed.len(),
            1,
            "the turn must still be registered after the caller's future was dropped"
        );
        assert_eq!(
            observed[0].session_id,
            handoff_turn_session_id("zig", Some("Bot Chat"))
        );
        assert_eq!(
            observed[0].surface.to_string(),
            ironhermes_core::Surface::Cli.to_string()
        );

        match crate::server::handoff_steering::begin_turn_or_queue("zig", "second send") {
            Ok(crate::server::handoff_steering::BeginOrQueue::Queued { .. }) => {}
            other => panic!("the bot must still read as busy after the dropped future — got {other:?}"),
        }

        let turn_id = observed[0].turn_id;
        let cancelled = registry_for_poll.cancel_one(turn_id).await;
        assert!(cancelled, "cancel_one must find the surviving turn");

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if registry_for_poll.list_all().await.is_empty() {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the registry must drain to empty within the bounded poll window after cancellation"
            );
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }

        match crate::server::handoff_steering::begin_turn_or_queue("zig", "third send") {
            Ok(crate::server::handoff_steering::BeginOrQueue::Begin(guard)) => drop(guard),
            other => panic!(
                "the bot must read as idle again once the subprocess is drained — got {other:?}"
            ),
        }
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn awaiting_the_detached_bot_dispatch_to_completion_matches_the_inline_result() {
        let _lock = crate::server::test_support::env_lock();
        crate::server::handoff_steering::reset_steering_for_test();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        scaffold_profile_workspace("zig");
        let stub = write_stub_script(
            dir.path(),
            "stub-detached-complete.sh",
            "echo 'detached reply text'",
        );
        let _bin_guard = ScopedEnv::set(
            "IRONHERMES_WORKER_BIN",
            stub.to_str().expect("utf8 stub path"),
        );

        let registry = ironhermes_core::TurnRegistry::new();
        let registry_for_poll = registry.clone();
        let in_flight = crate::server::handoff_steering::mark_bot_in_flight("zig");

        let result = dispatch_bot_message_detached_in(
            registry,
            in_flight,
            "zig".to_string(),
            "hello".to_string(),
            Some("Bot Chat".to_string()),
        )
        .await;
        assert_eq!(
            result.expect("detached dispatch should Ok").reply,
            "detached reply text"
        );

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if registry_for_poll.list_all().await.is_empty() {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the registry must drain to empty once the detached dispatch completes"
            );
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }

    // -------------------------------------------------------------------
    // dispatch_bot_message steering (Phase 50.2 Plan 16, Task 1, G-50.2-2c)
    // -------------------------------------------------------------------

    /// `$5` is the `-q` prompt value. The stub appends it followed by a
    /// delimiter line to `capture_path`, so a caller can split the file's
    /// content on the delimiter to recover exactly one block per subprocess
    /// invocation — a plain newline split would be wrong here because a
    /// FOLDED prompt (Task 1's own `fold_steering_into_prompt`) legitimately
    /// contains embedded newlines within a single `$5` value.
    const STEERING_TEST_PROMPT_DELIMITER: &str = "===GSD-50.2-16-PROMPT-END===";

    fn steering_test_stub_body(capture_path: &Path, reply: &str) -> String {
        format!(
            "printf '%s\\n' \"$5\" >> {capture}\nprintf '{delim}\\n' >> {capture}\nsleep 2\necho '{reply}'",
            capture = capture_path.to_str().expect("utf8 capture path"),
            delim = STEERING_TEST_PROMPT_DELIMITER,
            reply = reply,
        )
    }

    fn steering_test_captured_prompts(capture_path: &Path) -> Vec<String> {
        let raw = std::fs::read_to_string(capture_path).unwrap_or_default();
        raw.split(&format!("{STEERING_TEST_PROMPT_DELIMITER}\n"))
            .map(|s| s.trim_end_matches('\n').to_string())
            .filter(|s| !s.is_empty())
            .collect()
    }

    fn steering_test_write_gate_config(dir: &Path) {
        std::fs::write(
            dir.join("config.yaml"),
            "security:\n  web_config_write_enabled: true\n",
        )
        .expect("write config.yaml");
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn mid_flight_send_is_queued_instead_of_spawning_a_second_subprocess() {
        let _lock = crate::server::test_support::env_lock();
        crate::server::handoff_steering::reset_steering_for_test();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        scaffold_profile_workspace("zig");
        steering_test_write_gate_config(dir.path());

        let capture_path = dir.path().join("capture.txt");
        let stub = write_stub_script(
            dir.path(),
            "stub-steering-mid-flight.sh",
            &steering_test_stub_body(&capture_path, "first reply"),
        );
        let _bin_guard = ScopedEnv::set(
            "IRONHERMES_WORKER_BIN",
            stub.to_str().expect("utf8 stub path"),
        );

        let first = tokio::spawn(dispatch_bot_message(crate::protocol::BotHandoffRequest {
            name: "zig".to_string(),
            message: "first message".to_string(),
        }));

        // Poll until the first turn has genuinely started (its stub has
        // written its capture line) before sending the mid-flight second
        // message — a fixed sleep could race the child's own startup.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while std::time::Instant::now() < deadline
            && steering_test_captured_prompts(&capture_path).is_empty()
        {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert!(
            !steering_test_captured_prompts(&capture_path).is_empty(),
            "the first turn must have started before the mid-flight send"
        );

        let second = dispatch_bot_message(crate::protocol::BotHandoffRequest {
            name: "zig".to_string(),
            message: "second message (steering)".to_string(),
        })
        .await
        .expect("G-50.2-2c: a mid-flight send must be accepted, not error");

        assert_eq!(
            second,
            crate::protocol::BotDispatchOutcome::Queued { depth: 1 },
            "G-50.2-2c: a mid-flight send to a busy bot must be queued at depth 1, not spawn a second subprocess"
        );

        let first_result = first
            .await
            .expect("join must succeed")
            .expect("G-50.2-2c: the first (run-now) dispatch must Ok");
        match first_result {
            crate::protocol::BotDispatchOutcome::Replied(r) => {
                assert_eq!(r.reply, "first reply");
            }
            other => panic!("G-50.2-2c: expected Replied for the first dispatch, got a Queued outcome instead: {other:?}"),
        }

        let prompts = steering_test_captured_prompts(&capture_path);
        assert_eq!(
            prompts.len(),
            1,
            "G-50.2-2c: exactly ONE prompt must have reached the child — the mid-flight send must never spawn a second subprocess; captured={prompts:?}"
        );
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn queued_steering_message_is_folded_into_the_next_prompt() {
        let _lock = crate::server::test_support::env_lock();
        crate::server::handoff_steering::reset_steering_for_test();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        scaffold_profile_workspace("zig");
        steering_test_write_gate_config(dir.path());

        let capture_path = dir.path().join("capture.txt");
        let stub = write_stub_script(
            dir.path(),
            "stub-steering-fold.sh",
            &steering_test_stub_body(&capture_path, "reply"),
        );
        let _bin_guard = ScopedEnv::set(
            "IRONHERMES_WORKER_BIN",
            stub.to_str().expect("utf8 stub path"),
        );

        let first = tokio::spawn(dispatch_bot_message(crate::protocol::BotHandoffRequest {
            name: "zig".to_string(),
            message: "first message".to_string(),
        }));

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while std::time::Instant::now() < deadline
            && steering_test_captured_prompts(&capture_path).is_empty()
        {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert!(
            !steering_test_captured_prompts(&capture_path).is_empty(),
            "the first turn must have started before the mid-flight send"
        );

        let queued_outcome = dispatch_bot_message(crate::protocol::BotHandoffRequest {
            name: "zig".to_string(),
            message: "also check the logs".to_string(),
        })
        .await
        .expect("G-50.2-2c: the mid-flight send must be accepted");
        assert_eq!(
            queued_outcome,
            crate::protocol::BotDispatchOutcome::Queued { depth: 1 }
        );

        let _first_result = first
            .await
            .expect("join must succeed")
            .expect("G-50.2-2c: the first dispatch must Ok");

        let third = dispatch_bot_message(crate::protocol::BotHandoffRequest {
            name: "zig".to_string(),
            message: "fresh message".to_string(),
        })
        .await
        .expect("G-50.2-2c: the third dispatch must Ok");
        match third {
            crate::protocol::BotDispatchOutcome::Replied(_) => {}
            other => panic!("G-50.2-2c: expected Replied for the third dispatch, got {other:?}"),
        }

        let prompts = steering_test_captured_prompts(&capture_path);
        let last_prompt = prompts
            .last()
            .expect("G-50.2-2c: at least one prompt must have been captured");
        assert!(
            last_prompt.contains("also check the logs"),
            "G-50.2-2c: the queued steering text must be folded into the next prompt, got={last_prompt:?}"
        );
        assert!(
            last_prompt.contains("fresh message"),
            "G-50.2-2c: the fresh message must still be present verbatim, got={last_prompt:?}"
        );
        assert_eq!(
            crate::server::handoff_steering::steering_depth("zig"),
            0,
            "G-50.2-2c: the queue must be empty once its contents have been drained into a turn"
        );
    }
}
