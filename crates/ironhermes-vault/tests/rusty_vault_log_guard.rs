//! Phase 51 Plan 01 Task 3 (T-51-01, U-4) — re-proves the `rusty_vault=off` `EnvFilter`
//! directive against the surviving 0.3.1 leak, sitting next to the vault crate itself rather
//! than only in the CLI (`nf1_rusty_vault_log_silence_46_8_gap` in
//! `crates/ironhermes-cli/src/main.rs`, which this file's capturing-layer harness is modelled
//! on).
//!
//! The 0.2.1 -> 0.3.1 migration genuinely fixed the master-key/unseal-share leak
//! (`core.rs` at the pinned rev has exactly one static-string `log::error!` in the whole
//! file) — but `rusty_vault::modules::auth::token_store::check_token` still does
//! `log::debug!("check token: {token}")` with the RAW client bearer token, unchanged
//! 0.2.1 -> 0.3.1 (only the string-interpolation syntax was modernized). That function fires
//! on every authenticated request, including every read Phase 51's later plans route through
//! the new worker-facing endpoint — so this is not a hypothetical, it is the exact code path a
//! profile-scoped worker token will hit on every dispatch.
//!
//! Does NOT require `--features rusty-vault` — this test never constructs a real `Core`; it
//! only proves the `tracing-subscriber` `EnvFilter` composition mechanism the three production
//! sites share (`crates/ironhermes-cli/src/main.rs:1922`,
//! `crates/iron_hermes_ui/src/server/logging.rs:60`,
//! `crates/ironhermes-cli/src/tui_rata/event_loop.rs:96`), using a synthetic `tracing::debug!`
//! call shaped like `token_store::check_token`'s real one.

use std::sync::{Arc, Mutex};

use tracing_subscriber::Layer;
use tracing_subscriber::layer::SubscriberExt;

/// The real target `log::debug!` in `token_store.rs` emits on, via `module_path!()` (the `log`
/// crate's default target) and the `tracing-log` bridge that carries it into whichever
/// subscriber is installed.
const TOKEN_STORE_TARGET: &str = "rusty_vault::modules::auth::token_store";

/// A distinctive sentinel standing in for a real client bearer token — never a real credential.
const SENTINEL_TOKEN: &str = "TOKEN-DEBUG-LEAK-SENTINEL-9f31c2";

/// Minimal capturing layer: records `target::message` for every event that actually reaches it
/// (i.e. survived the upstream `EnvFilter`), so the assertion is on real subscriber-observed
/// output, not just on a directive string. Mirrors `main.rs`'s `CaptureLayer` exactly.
#[derive(Clone, Default)]
struct CaptureLayer(Arc<Mutex<Vec<String>>>);

struct MessageVisitor(String);
impl tracing::field::Visit for MessageVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.0 = format!("{value:?}");
        }
    }
}

impl<S: tracing::Subscriber> Layer<S> for CaptureLayer {
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: tracing_subscriber::layer::Context<'_, S>) {
        let mut visitor = MessageVisitor(String::new());
        event.record(&mut visitor);
        self.0
            .lock()
            .unwrap()
            .push(format!("{}::{}", event.metadata().target(), visitor.0));
    }
}

/// Emit the two sentinel events (token-shaped debug on the real `token_store` target, plus an
/// unrelated `ironhermes` info event for the sanity check) under `subscriber`.
fn emit_sentinel_events(subscriber: impl tracing::Subscriber + Send + Sync) {
    tracing::subscriber::with_default(subscriber, || {
        tracing::debug!(target: TOKEN_STORE_TARGET, "check token: {SENTINEL_TOKEN}");
        tracing::info!(target: "ironhermes", "unrelated info event");
    });
}

/// T-51-01 / U-4 — GREEN: mirrors the production composition order at all three sites (the
/// `RUST_LOG`-derived filter first, `rusty_vault=off` appended AFTER) with an operator-supplied
/// `RUST_LOG` that would enable `rusty_vault` debug logs absent the fix. The token-shaped debug
/// event on the real `check_token` target must never reach the capturing layer.
///
/// RED evidence (captured before this assertion was restored to match production — see this
/// plan's SUMMARY.md for the full captured output): building this exact filter WITHOUT the
/// trailing `.add_directive("rusty_vault=off"...)` call lets the sentinel event surface
/// verbatim in `events`, proving the leak is real absent the directive this test now asserts
/// is present.
#[test]
fn token_store_check_token_debug_event_is_suppressed() {
    let captured: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let capture_layer = CaptureLayer(captured.clone());

    // Mirror production composition order EXACTLY: RUST_LOG-derived filter first, then
    // `rusty_vault=off` appended after (crates/ironhermes-cli/src/main.rs:1922,
    // crates/iron_hermes_ui/src/server/logging.rs:60,
    // crates/ironhermes-cli/src/tui_rata/event_loop.rs:96).
    let env_filter = tracing_subscriber::EnvFilter::new("rusty_vault=debug,ironhermes=info")
        .add_directive("rusty_vault=off".parse().expect("valid static directive"));

    let subscriber = tracing_subscriber::registry().with(env_filter).with(capture_layer);
    emit_sentinel_events(subscriber);

    let events = captured.lock().unwrap();
    assert!(
        !events.iter().any(|e| e.contains(SENTINEL_TOKEN)),
        "token_store::check_token's raw-bearer-token debug event leaked through despite the \
         `rusty_vault=off` directive: {events:?}"
    );
    assert!(
        events.iter().any(|e| e.starts_with("ironhermes::")),
        "sanity check failed: the filter suppressed everything, not just the rusty_vault \
         target: {events:?}"
    );
}

/// T-51-57 — pins the composition ORDER that makes the three production sites correct: with
/// the suppression directive applied BEFORE the `RUST_LOG`-derived filter (the wrong order), a
/// later `.add_directive` for the `RUST_LOG`-derived target list is what actually takes final
/// effect for overlapping targets, and the sentinel DOES surface. A future refactor that
/// reorders the production sites' filter construction fails here instead of silently in
/// production.
#[test]
fn directive_order_matters() {
    let captured: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let capture_layer = CaptureLayer(captured.clone());

    // WRONG order: suppression directive first, RUST_LOG-derived directives applied after.
    let env_filter = tracing_subscriber::EnvFilter::new("rusty_vault=off")
        .add_directive("rusty_vault=debug".parse().expect("valid static directive"))
        .add_directive("ironhermes=info".parse().expect("valid static directive"));

    let subscriber = tracing_subscriber::registry().with(env_filter).with(capture_layer);
    emit_sentinel_events(subscriber);

    let events = captured.lock().unwrap();
    assert!(
        events.iter().any(|e| e.contains(SENTINEL_TOKEN)),
        "expected the sentinel to surface when the suppression directive is applied BEFORE the \
         RUST_LOG-derived filter (proving composition ORDER is what makes the three production \
         sites correct), but it was suppressed: {events:?}"
    );
}

// ---------------------------------------------------------------------------
// T-51-57 (Phase 51 Plan 12) — per-site occurrence tripwire. The two tests
// above prove the suppression mechanism WORKS when the directive is present;
// this one proves the directive is still INSTALLED at each of the three
// production subscriber-install sites. Neither replaces the other.
// ---------------------------------------------------------------------------

/// `CARGO_MANIFEST_DIR` for this crate is `<root>/crates/ironhermes-vault` — two
/// levels up reaches the workspace root.
fn workspace_root_for_log_guard() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("ironhermes-vault's manifest dir must have a parent (crates/)")
        .parent()
        .expect("crates/ must have a parent (the workspace root)")
        .to_path_buf()
}

/// Strip whole-line `//` comments before searching — a bare occurrence count
/// would let a file pass on its own explanatory prose (a doc comment merely
/// *mentioning* the directive) after the live directive was deleted. Mirrors
/// `ironhermes-core/tests/dispatch_gate_vault_backed.rs`'s `strip_comment_lines`.
fn strip_comment_lines(src: &str) -> String {
    src.lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// One subscriber-install site: its path relative to `crates/`, a human
/// description of what it installs (folded into the failure message so the
/// next reader knows which surface just lost its guard), and an optional
/// marker: when present, only the file slice BEFORE that marker is searched.
/// `main.rs` carries its own `#[cfg(test)]` module
/// (`nf1_rusty_vault_log_silence_46_8_gap`) that legitimately re-quotes the
/// directive as part of ITS OWN test fixture — without this boundary, that
/// fixture's copy would mask a deleted PRODUCTION directive.
struct SubscriberSite {
    path: &'static str,
    installs: &'static str,
    production_only_boundary: Option<&'static str>,
}

const SUBSCRIBER_SITES: &[SubscriberSite] = &[
    SubscriberSite {
        path: "ironhermes-cli/src/main.rs",
        installs: "the `ironhermes` CLI's tracing subscriber (interactive + \
                   non-interactive entry points)",
        production_only_boundary: Some("mod nf1_rusty_vault_log_silence_46_8_gap"),
    },
    SubscriberSite {
        path: "iron_hermes_ui/src/server/logging.rs",
        installs: "iron_hermes_ui's web server tracing subscriber",
        production_only_boundary: None,
    },
    SubscriberSite {
        path: "ironhermes-cli/src/tui_rata/event_loop.rs",
        installs: "the ratatui TUI event loop's tracing subscriber",
        production_only_boundary: None,
    },
];

#[test]
fn rusty_vault_off_directive_is_installed_at_all_three_subscriber_sites() {
    let root = workspace_root_for_log_guard();
    // Assembled at runtime from two halves rather than written as one literal,
    // so this test file cannot satisfy its own search.
    let needle = {
        let lhs = "rusty_vault=";
        let rhs = "off";
        format!("{lhs}{rhs}")
    };

    for site in SUBSCRIBER_SITES {
        let full_path = root.join("crates").join(site.path);
        let full_src = std::fs::read_to_string(&full_path)
            .unwrap_or_else(|e| panic!("read {}: {e}", site.path));
        let scoped_src = match site.production_only_boundary {
            Some(marker) => {
                let idx = full_src.find(marker).unwrap_or_else(|| {
                    panic!(
                        "{}: expected boundary marker {marker:?} to exist in the file — \
                         has it moved or been renamed?",
                        site.path
                    )
                });
                &full_src[..idx]
            }
            None => full_src.as_str(),
        };
        let stripped = strip_comment_lines(scoped_src);

        let occurrences = stripped.matches(&needle).count();
        assert!(
            occurrences >= 1,
            "{}: no non-comment occurrence of the `{needle}` directive found. This site \
             installs {} — `rusty_vault::modules::auth::token_store::check_token` \
             debug-logs the RAW client bearer token on every authenticated request, so \
             dropping this directive here re-opens a plaintext token leak under that \
             crate's feature plus a debug log level.",
            site.path, site.installs
        );
    }
}
