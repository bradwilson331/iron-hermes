//! Phase 36.3.12 Task 2 (D-08/D-10/D-11/D-12, GAP 2 / WR-02): workspace-wide
//! structural sweep — no production `TurnRequest` construction site may leave
//! `terminal_intercept` or `execute_code_intercept` unset.
//!
//! # Why this exists (two prior recurrences)
//!
//! The web surface (`iron_hermes_ui::server::state::run_web_turn`) left both
//! intercept fields unset — twice. First when Phase 45 D-11 added the fields
//! (the surface's own comment admitted "was missed when D-11 added these
//! fields"), then again in this phase (GAP 2 in
//! `36.3.12-VERIFICATION.md`), before Plan 12 Task 1 wired it. WR-02 (this
//! phase's REVIEW.md) separately records an example of a test that *looked*
//! like it guarded the wiring but did not actually drive it. This sweep exists
//! to make a THIRD recurrence — on this surface or a brand new one —
//! structurally impossible to ship unnoticed.
//!
//! # Why it walks the filesystem instead of using a fixed file list
//!
//! A hardcoded list of "known" files would miss exactly the failure mode that
//! has happened twice: a NEW surface adding a `TurnRequest` construction that
//! nobody remembered to add to the list. Enumerating from disk (`crates/*/src/`)
//! means a future surface is covered automatically, with no sweep-maintenance
//! step required.
//!
//! # Why it reads source text instead of relying on compilation
//!
//! `iron_hermes_ui` is excluded from `default-members` and its server code
//! (including `run_web_turn`) sits behind the `server` feature. A sweep that
//! only sees what the *current* feature set compiles would let this exact
//! surface hide again — which is precisely why it escaped twice. A filesystem
//! text sweep is immune to feature flags and workspace member exclusion. This
//! generalizes the established `include_str!` source-invariant precedent in
//! this repo (`crates/ironhermes-cli/tests/invariants_22_4.rs`'s
//! INV-21.7/22.1/22.3/22.4 constants, including its cross-crate
//! `CORE_REGISTRY` inclusion) from a fixed set of `include_str!` constants to a
//! recursive walk — the same "read the text, not the compiled artifact"
//! philosophy, generalized so it cannot be defeated by adding a new file.
//!
//! # Why `approval_gate` is deliberately NOT asserted on
//!
//! The CLI's three `TurnRequest` sites (`main.rs`) and the TUI's one
//! (`tui_rata/event_loop.rs`) all legitimately leave `approval_gate` unset
//! while being fully, correctly gated — because `CliApprovalGate` is
//! constructed *inside* their intercept closures rather than passed through
//! the `TurnRequest.approval_gate` field (see
//! `crates/ironhermes-cli/src/approval_gate.rs`). The `approval_gate` field
//! feeds `AgentLoop`'s own `NeedsApproval` arm for *other* tools; the gateway
//! supplies it there, CLI/TUI legitimately do not. Asserting "no site leaves
//! `approval_gate` unset" would fail on four correctly-gated sites and would
//! have to be neutered to pass — turning the sweep into theater. The two
//! INTERCEPT fields are the actual D-08/D-10 chokepoint, and they are what
//! must never be unset on a production site. **Do not "helpfully" add an
//! `approval_gate` assertion here** — it will break four sites that are
//! already correct.
//!
//! # Why `#[cfg(test)]` exclusion is region-based, not "truncate at the first
//! occurrence in the file"
//!
//! A naive "cut the file's text at the first `#[cfg(test)]`" breaks on real
//! files in this workspace: `ironhermes-cli/src/main.rs` and
//! `ironhermes-gateway/src/handler.rs` both attach `#[cfg(test)]` to small
//! helper items (a test-only `use`, a couple of `pub(crate) fn` mutex-guard
//! helpers) hundreds of lines *before* their real production `TurnRequest`
//! construction — truncating at the first occurrence would discard those
//! constructions entirely and make the sweep blind to them. Instead,
//! [`cfg_test_regions`] computes the exact byte range gated by each
//! `#[cfg(test)]` attribute (the attribute itself through its item's closing
//! `{ ... }` brace, or through its terminating `;` for a body-less item like
//! `use`/`const`), and a `TurnRequest {` occurrence is excluded only when it
//! falls inside one of those precise ranges.

use std::path::{Path, PathBuf};

// ─────────────────────────────────────────────────────────────────────────────
// Pure scanning helpers — unit-tested directly against synthetic content below,
// and exercised against the real workspace tree by the `#[test]` fns.
// ─────────────────────────────────────────────────────────────────────────────

/// Resolve the workspace root from `CARGO_MANIFEST_DIR`
/// (`<root>/crates/ironhermes-agent`), going up two levels.
fn workspace_root() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent() // crates/
        .and_then(|p| p.parent()) // workspace root
        .expect("CARGO_MANIFEST_DIR must be <root>/crates/ironhermes-agent")
        .to_path_buf()
}

fn is_ident_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// `true` if `path` has a `tests` path component anywhere (excludes both
/// crate-level `tests/` integration-test directories and, defensively, any
/// `src/tests/` subdirectory some crate might add in the future).
fn is_excluded_path(path: &Path) -> bool {
    path.components()
        .any(|c| c.as_os_str() == std::ffi::OsStr::new("tests"))
}

/// Brace-depth scan from an opening `{` (at `open_brace_idx`) to its matching
/// closing `}`. No parser needed — Rust source has balanced braces outside of
/// string/char literals and comments, and none of the five known production
/// sites (nor any plausible new one) construct a `TurnRequest` inside a string
/// or comment.
fn find_matching_close(content: &str, open_brace_idx: usize) -> Option<usize> {
    let bytes = content.as_bytes();
    if bytes.get(open_brace_idx) != Some(&b'{') {
        return None;
    }
    let mut depth: i32 = 0;
    let mut i = open_brace_idx;
    while i < bytes.len() {
        match bytes[i] {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Byte ranges `[start, end)` covered by every `//` line comment or `/* */`
/// block comment in `span` (WR-08, Phase 36.3.12 review round 2).
///
/// Naive (no nested-block-comment support, no string-literal awareness) —
/// same simplifying assumption [`find_matching_close`]'s doc already states
/// for this domain: none of the five known production `TurnRequest`
/// construction sites (nor any plausible new one) put comment delimiters
/// inside a string/char literal within the construction's own span. Used by
/// [`field_is_unset`] to make sure a bare mention of a field name inside a
/// comment — BEFORE the field's real assignment further down — is never
/// mistaken for the field itself.
fn comment_ranges(span: &str) -> Vec<(usize, usize)> {
    let bytes = span.as_bytes();
    let mut ranges = Vec::new();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'/' && bytes.get(i + 1) == Some(&b'/') {
            let start = i;
            let end = span[i..].find('\n').map_or(span.len(), |p| i + p);
            ranges.push((start, end));
            i = end;
        } else if bytes[i] == b'/' && bytes.get(i + 1) == Some(&b'*') {
            let start = i;
            let end = span[i + 2..]
                .find("*/")
                .map_or(span.len(), |p| i + 2 + p + 2);
            ranges.push((start, end));
            i = end;
        } else {
            i += 1;
        }
    }
    ranges
}

/// `true` if `field` (e.g. `"terminal_intercept"`) is either absent from
/// `span` entirely, or present with the literal value `None` — i.e. left
/// unset on this `TurnRequest { ... }` construction.
///
/// Handles both assignment forms seen in production: `field: Some(...)` /
/// `field: None`, and Rust's field-init shorthand `field,` (used by
/// `ironhermes-gateway::handler`, which builds a local `let terminal_intercept
/// = ...;` and passes it by shorthand) — the shorthand form is always treated
/// as constructed, since a bare identifier can only appear there if some
/// earlier binding built it (a literal `None` can't be written as bare
/// shorthand).
///
/// # Comment-mention hardening (WR-08, Phase 36.3.12 review round 2)
///
/// Two defenses against a bare mention of the field name inside a `//` or
/// `/* */` comment preceding the real assignment:
///
/// 1. Any whole-token match falling inside a [`comment_ranges`] span is
///    skipped outright — the scan keeps looking for the next occurrence.
/// 2. Even outside a comment, the "shorthand field-init" classification now
///    requires the next non-whitespace token to be exactly `,` or `}` — the
///    pre-fix version accepted ANYTHING that was not immediately `:`,
///    which would also (mis)classify a field name followed by ordinary prose
///    as "constructed" if it ever escaped defense 1 (e.g. inside a doc
///    comment form this scanner does not yet recognize).
fn field_is_unset(span: &str, field: &str) -> bool {
    let bytes = span.as_bytes();
    let comments = comment_ranges(span);
    let in_comment = |pos: usize| comments.iter().any(|(s, e)| pos >= *s && pos < *e);
    let mut idx = 0usize;
    loop {
        let Some(rel) = span.get(idx..).and_then(|s| s.find(field)) else {
            // Field never appears (outside a comment) in this construction → unset.
            return true;
        };
        let abs = idx + rel;
        let before_ok = abs == 0 || !is_ident_char(bytes[abs - 1]);
        let after_idx = abs + field.len();
        let after_ok = after_idx >= bytes.len() || !is_ident_char(bytes[after_idx]);
        if before_ok && after_ok && !in_comment(abs) {
            // Whole-token match on the field name itself (not a substring of
            // a longer identifier), and not a bare mention inside a comment.
            let mut j = after_idx;
            while j < bytes.len() && (bytes[j] as char).is_whitespace() {
                j += 1;
            }
            if bytes.get(j) == Some(&b':') {
                j += 1;
                while j < bytes.len() && (bytes[j] as char).is_whitespace() {
                    j += 1;
                }
                let rest = &span[j..];
                let is_none_literal = rest.starts_with("None")
                    && rest
                        .as_bytes()
                        .get(4)
                        .map(|c| !is_ident_char(*c))
                        .unwrap_or(true);
                return is_none_literal;
            }
            if matches!(bytes.get(j), Some(b',') | Some(b'}')) {
                // Shorthand field-init (`field,` / `field }`) — constructed.
                return false;
            }
            // Whole-token match, not inside a comment, but not followed by
            // `:` / `,` / `}` either — not a genuine field reference. Keep
            // scanning for the real occurrence.
        }
        idx = abs + field.len();
    }
}

/// Byte ranges `[start, end)` gated by every `#[cfg(test)]` attribute in
/// `content` — from the attribute itself through the end of the item it
/// gates. Handles both brace-bodied items (`mod`/`fn`/`impl`/`trait`/
/// `struct { }`/`enum { }` — region ends at the matching `}`) and body-less
/// items (`use`/`const`/`static`/`type` — region ends at the terminating
/// top-level `;`), and skips over any additional stacked attributes
/// (`#[cfg(test)]\n#[allow(dead_code)]\nfn ...`) before determining which.
///
/// See the module doc ("region-based, not truncate-at-first-occurrence") for
/// why this exists instead of the simpler whole-file truncation.
fn cfg_test_regions(content: &str) -> Vec<(usize, usize)> {
    let bytes = content.as_bytes();
    let needle = "#[cfg(test)]";
    let mut regions = Vec::new();
    let mut search_from = 0usize;

    while let Some(rel) = content.get(search_from..).and_then(|s| s.find(needle)) {
        let attr_start = search_from + rel;
        let mut i = attr_start + needle.len();

        // Skip whitespace and any further stacked `#[...]` attributes — they
        // gate the SAME item as the `#[cfg(test)]` we just found.
        loop {
            while i < bytes.len() && (bytes[i] as char).is_whitespace() {
                i += 1;
            }
            if content[i..].starts_with('#')
                && let Some(close_rel) = content[i..].find(']')
            {
                i += close_rel + 1;
                continue;
            }
            break;
        }

        // Scan forward tracking (){}[] depth to find this item's own
        // top-level `{` (a body) or top-level `;` (no body), whichever comes
        // first — e.g. `fn foo<T>() -> Bar<Baz> { ... }` stays at depth 0
        // through the `()`/generic segments until its real body brace; a
        // body-less `use foo::{bar, baz};` hits the top-level `;` because the
        // `{bar, baz}` is depth 1, not 0.
        let mut depth: i32 = 0;
        let mut j = i;
        let mut region_end = content.len();
        while j < bytes.len() {
            match bytes[j] {
                b'{' if depth == 0 => {
                    region_end = find_matching_close(content, j).map_or(content.len(), |c| c + 1);
                    break;
                }
                b'{' | b'(' | b'[' => depth += 1,
                b'}' | b')' | b']' => depth -= 1,
                b';' if depth == 0 => {
                    region_end = j + 1;
                    break;
                }
                _ => {}
            }
            j += 1;
        }

        regions.push((attr_start, region_end));
        search_from = region_end.max(i);
    }
    regions
}

/// Scan one file's already-read source text for production
/// `TurnRequest { ... }` constructions, returning each construction's full
/// text span (from `TurnRequest {` to its matching `}`, inclusive).
///
/// Two exclusions, per the module doc:
/// - Skips any occurrence whose start falls inside a [`cfg_test_regions`]
///   range, so in-file test-module (and test-gated-item) constructions are
///   never considered.
/// - Skips any `TurnRequest {` occurrence preceded on the same line by
///   `struct` — the struct *definition* (`agent_runtime.rs`'s
///   `pub struct TurnRequest {`), not a construction.
fn find_constructions_in_content(content: &str) -> Vec<String> {
    let test_regions = cfg_test_regions(content);
    let in_test_region = |pos: usize| test_regions.iter().any(|(s, e)| pos >= *s && pos < *e);

    let needle = "TurnRequest {";
    let mut results = Vec::new();
    let mut search_from = 0usize;
    while let Some(rel) = content.get(search_from..).and_then(|s| s.find(needle)) {
        let match_start = search_from + rel;
        let brace_idx = match_start + needle.len() - 1;

        let line_start = content[..match_start]
            .rfind('\n')
            .map(|p| p + 1)
            .unwrap_or(0);
        let line_prefix = &content[line_start..match_start];

        if !line_prefix.contains("struct")
            && !in_test_region(match_start)
            && let Some(close_idx) = find_matching_close(content, brace_idx)
        {
            results.push(content[match_start..=close_idx].to_string());
        }
        search_from = match_start + needle.len();
    }
    results
}

fn collect_rs_files_under(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rs_files_under(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs")
            && !is_excluded_path(&path)
        {
            out.push(path);
        }
    }
}

/// Every `.rs` file under `crates/*/src/` in the workspace — enumerated from
/// disk, not a fixed list (module doc: "why it walks the filesystem").
fn production_rs_files() -> Vec<PathBuf> {
    let crates_dir = workspace_root().join("crates");
    let mut files = Vec::new();
    let Ok(entries) = std::fs::read_dir(&crates_dir) else {
        return files;
    };
    for entry in entries.flatten() {
        let crate_dir = entry.path();
        if !crate_dir.is_dir() {
            continue;
        }
        let src_dir = crate_dir.join("src");
        if src_dir.is_dir() {
            collect_rs_files_under(&src_dir, &mut files);
        }
    }
    files
}

/// Crates every structural sweep skips: `ironagent-tools-api` is a workspace-excluded
/// stale duplicate whose registry and gating code never ships. Shared by every check so
/// no sweep silently reads a crate another one skips.
const SKIPPED_CRATE_DIRS: &[&str] = &["ironagent-tools-api"];

/// Every `.rs` file under `crates/*/src/` except [`SKIPPED_CRATE_DIRS`], sorted.
fn swept_rs_files() -> Vec<PathBuf> {
    let crates_dir = workspace_root().join("crates");
    let mut files = Vec::new();
    let Ok(entries) = std::fs::read_dir(&crates_dir) else {
        return files;
    };
    for entry in entries.flatten() {
        let crate_dir = entry.path();
        if !crate_dir.is_dir() {
            continue;
        }
        if crate_dir
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| SKIPPED_CRATE_DIRS.contains(&n))
        {
            continue; // workspace-excluded stale duplicate — never scanned
        }
        let src_dir = crate_dir.join("src");
        if src_dir.is_dir() {
            collect_rs_files_under(&src_dir, &mut files);
        }
    }
    files.sort();
    files
}

/// `crates/<crate>/src/...` with forward slashes, for exemption matching and messages.
fn rel_label(file: &Path) -> String {
    file.strip_prefix(workspace_root())
        .unwrap_or(file)
        .to_string_lossy()
        .replace('\\', "/")
}

/// `(label, content)` for every swept file that reads as UTF-8.
fn swept_sources() -> Vec<(String, String)> {
    swept_rs_files()
        .into_iter()
        .filter_map(|f| std::fs::read_to_string(&f).ok().map(|c| (rel_label(&f), c)))
        .collect()
}

/// Every production `TurnRequest` construction site across the workspace, as
/// `(file, construction_span_text)` pairs.
fn all_production_constructions() -> Vec<(PathBuf, String)> {
    let mut out = Vec::new();
    for file in production_rs_files() {
        let Ok(content) = std::fs::read_to_string(&file) else {
            continue;
        };
        for span in find_constructions_in_content(&content) {
            out.push((file.clone(), span));
        }
    }
    out
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

/// The load-bearing sweep: every production `TurnRequest` construction site
/// wires BOTH `terminal_intercept` and `execute_code_intercept` to a
/// constructed value. Deliberately does NOT assert on `approval_gate` — see
/// the module doc's "why `approval_gate` is deliberately NOT asserted on".
///
/// Against pre-Task-1 code this test FAILS, naming
/// `crates/iron_hermes_ui/src/server/state.rs` (the mandatory falsification
/// check — see Plan 12's SUMMARY.md for the observed failure message).
#[test]
fn every_production_turn_request_site_wires_both_intercepts() {
    let sites = all_production_constructions();
    assert!(
        !sites.is_empty(),
        "the sweep found ZERO TurnRequest construction sites — the walk is broken (wrong \
         workspace root, bad file glob) and this test would otherwise pass vacuously. See \
         `sweep_actually_sees_the_known_surfaces` for the meta-assertion that should have \
         caught this."
    );

    let mut failures = Vec::new();
    for (file, span) in &sites {
        for field in ["terminal_intercept", "execute_code_intercept"] {
            if field_is_unset(span, field) {
                failures.push(format!(
                    "{}: `{field}` is unset (or missing) on a production TurnRequest \
                     construction — this bypasses the D-08/D-10 guardrail+audit chokepoint \
                     (ironhermes_hooks::execute_gated_command). Mirror \
                     crates/ironhermes-cli/src/approval_gate.rs's \
                     build_gated_terminal_intercept / build_gated_execute_code_intercept \
                     builders to fix.",
                    file.display()
                ));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "found production TurnRequest site(s) bypassing the D-08/D-10 chokepoint:\n{}",
        failures.join("\n")
    );
}

/// Meta-assertion: the walk actually discovered the known production surfaces
/// — including `state.rs`, proving it reaches `iron_hermes_ui` despite that
/// crate's `default-members` exclusion and `server`-feature gating. A walk
/// that silently found nothing (wrong root, bad glob) would make the main
/// sweep test above pass vacuously; this test fails loudly instead.
#[test]
fn sweep_actually_sees_the_known_surfaces() {
    let sites = all_production_constructions();
    assert!(
        sites.len() >= 5,
        "expected at least 5 known production TurnRequest sites (gateway handler.rs, ui \
         state.rs, cli main.rs x3, cli tui_rata/event_loop.rs) — found {}. The walk root or \
         file glob is likely wrong.",
        sites.len()
    );

    let file_names: Vec<String> = sites.iter().map(|(f, _)| f.display().to_string()).collect();
    for expected_suffix in ["state.rs", "handler.rs", "main.rs", "event_loop.rs"] {
        assert!(
            file_names.iter().any(|n| n.ends_with(expected_suffix)),
            "the sweep did not discover a TurnRequest construction in a file ending with \
             `{expected_suffix}` — expected the walk to reach it regardless of \
             default-members exclusion or feature gating. Files found: {file_names:?}"
        );
    }
}

/// The walk excludes the struct *definition* and any construction inside a
/// `#[cfg(test)]` region.
///
/// The struct-definition exclusion is asserted against the REAL, on-disk
/// `agent_runtime.rs` (which a naive `TurnRequest {` text match would hit).
/// The `#[cfg(test)]` truncation is asserted against synthetic content,
/// because no real in-`src/`-tree file in this workspace currently constructs
/// a raw `TurnRequest { .. }` literal inside a `#[cfg(test)]` module (the
/// closest analog, `iron_hermes_ui::server::state`'s Task-1 tests, construct
/// intercept closures via helper functions rather than a literal
/// `TurnRequest { .. }`) — the synthetic case exercises the exclusion logic
/// directly and unambiguously.
#[test]
fn sweep_excludes_the_struct_definition_and_test_code() {
    // ── Real disk: the struct definition is not a construction ──────────────
    let sites = all_production_constructions();
    let agent_runtime_hits = sites
        .iter()
        .filter(|(f, _)| f.display().to_string().ends_with("agent_runtime.rs"))
        .count();
    assert_eq!(
        agent_runtime_hits, 0,
        "the `pub struct TurnRequest {{` DEFINITION in agent_runtime.rs must never be treated \
         as a construction site — got {agent_runtime_hits} hit(s)"
    );

    // ── Synthetic: a construction after #[cfg(test)] is excluded ────────────
    let synthetic = "\
let request = TurnRequest {
    terminal_intercept: Some(build_it()),
    execute_code_intercept: Some(build_it()),
};

#[cfg(test)]
mod tests {
    fn make() {
        let request = TurnRequest {
            terminal_intercept: None,
            execute_code_intercept: None,
        };
    }
}
";
    let found = find_constructions_in_content(synthetic);
    assert_eq!(
        found.len(),
        1,
        "exactly one construction (the one BEFORE #[cfg(test)]) must be found — the \
         in-test-module construction (deliberately left unwired in this synthetic fixture) \
         must be excluded by the #[cfg(test)] truncation. Got {} construction(s).",
        found.len()
    );
    assert!(
        !field_is_unset(&found[0], "terminal_intercept"),
        "the pre-#[cfg(test)] construction is fully wired and must not be flagged"
    );
    assert!(
        !field_is_unset(&found[0], "execute_code_intercept"),
        "the pre-#[cfg(test)] construction is fully wired and must not be flagged"
    );
}

/// Phase 54 Plan 02 (D-01, T-54-04): every production `execute_code` gating call
/// site — `ironhermes_hooks::execute_gated_command("execute_code", classify_arg,
/// ...)` — must pass REAL Python source as `classify_arg`, never an opaque
/// placeholder. Plan 01 wired the CLI (`ironhermes-cli/src/approval_gate.rs`);
/// Plan 02's Task 1 fanned the same fix out to the five remaining production
/// sites (ACP, gateway x2, web UI, CLI TUI). This is the drift probe: it walks
/// every `crates/*/src/**/*.rs` file (excluding `crates/ironagent-tools-api/`,
/// the workspace-excluded stale duplicate, and any `tests/` directory), strips
/// `//`/`/* */` comments and `#[cfg(test)]`-gated regions using this file's own
/// [`comment_ranges`] / [`cfg_test_regions`] helpers, and asserts every
/// remaining `execute_gated_command` call whose first argument names
/// `"execute_code"` passes a non-empty second argument.
///
/// The `#[cfg(test)]` exclusion is deliberate, not a loophole: two
/// crate-internal unit tests intentionally construct an EMPTY `classify_arg` to
/// prove the chokepoint's own generic Allow/Audit behavior when given one —
/// `ironhermes_tools::execute_code::execute_code_audit` and
/// `ironhermes_hooks::gated_exec::execute_code_empty_classify_arg_allows_and_audits`.
/// Neither is a production wiring site; counting them would make this probe
/// impossible to satisfy without breaking their own stated purpose.
#[test]
fn execute_code_gating_sites_pass_real_source() {
    let files = swept_rs_files();

    let needle = "execute_gated_command(";
    let mut sites = 0usize;
    let mut placeholders = Vec::new();

    for file in &files {
        let Ok(content) = std::fs::read_to_string(file) else {
            continue;
        };
        let comments = comment_ranges(&content);
        let in_comment = |pos: usize| comments.iter().any(|(s, e)| pos >= *s && pos < *e);
        let test_regions = cfg_test_regions(&content);
        let in_test = |pos: usize| test_regions.iter().any(|(s, e)| pos >= *s && pos < *e);

        let mut search_from = 0usize;
        while let Some(rel) = content.get(search_from..).and_then(|s| s.find(needle)) {
            let call_start = search_from + rel;
            let args_start = call_start + needle.len();
            search_from = args_start;

            if in_comment(call_start) || in_test(call_start) {
                continue;
            }

            let tail_end = (args_start + 400).min(content.len());
            let tail = &content[args_start..tail_end];
            let args: Vec<&str> = tail.split(',').map(str::trim).collect();
            if args.len() < 2 || !args[0].contains("execute_code") {
                continue;
            }

            sites += 1;
            let second = args[1].trim();
            if second == "\"\"" || second == "''" {
                placeholders.push(format!(
                    "{}: classify_arg is an empty string literal (call starts at byte {})",
                    file.display(),
                    call_start
                ));
            }
        }
    }

    assert!(
        sites >= 5,
        "expected at least 5 production execute_code gating call sites across \
         crates/*/src/ (excluding ironagent-tools-api and #[cfg(test)] regions) — \
         found {sites}. The probe's own matching may have broken (renamed \
         function, moved call shape) and would otherwise pass vacuously."
    );
    assert!(
        placeholders.is_empty(),
        "found execute_code gating site(s) still passing an opaque empty \
         classify_arg — DangerousCommandGuardrail's Python classification branch \
         (D-01) is a dead branch on these surfaces:\n{}",
        placeholders.join("\n")
    );
}

/// WR-08 (Phase 36.3.12 review round 2): a `//` or `/* */` comment INSIDE a
/// `TurnRequest { ... }` construction span that mentions `terminal_intercept`
/// or `execute_code_intercept` by name in prose — without an immediately
/// following `:` — appearing BEFORE the field's real `None` assignment
/// further down the span, must not cause `field_is_unset` to misclassify the
/// field as wired.
///
/// The pre-fix heuristic found only the FIRST textual occurrence of the
/// field name and, seeing it not immediately followed by `:`, treated it as
/// the shorthand field-init form (`field,`) — "constructed" — and returned
/// without ever looking for the real assignment further down. This
/// synthetic fixture plants exactly that defeat pattern and asserts the
/// sweep still correctly flags the field as unset.
#[test]
fn field_is_unset_is_not_defeated_by_a_preceding_comment_mention() {
    // Line-comment variant.
    let synthetic_line = "TurnRequest {
    // terminal_intercept must be wired here before this ships
    terminal_intercept: None,
    execute_code_intercept: None,
}";
    assert!(
        field_is_unset(synthetic_line, "terminal_intercept"),
        "a bare mention of `terminal_intercept` inside a preceding `//` comment must not \
         be mistaken for the shorthand field-init form — the field's REAL assignment \
         (`terminal_intercept: None`) further down the span must still be found and \
         correctly classified as unset"
    );

    // Block-comment variant of the same defeat attempt, on the OTHER field,
    // alongside a genuinely-wired sibling field (proving the sweep isn't just
    // failing everything once a comment is present).
    let synthetic_block = "TurnRequest {
    /* execute_code_intercept must be wired here before this ships */
    terminal_intercept: Some(build_it()),
    execute_code_intercept: None,
}";
    assert!(
        !field_is_unset(synthetic_block, "terminal_intercept"),
        "a genuinely-wired sibling field must still be classified as wired even when an \
         unrelated field is mentioned in a preceding block comment"
    );
    assert!(
        field_is_unset(synthetic_block, "execute_code_intercept"),
        "a bare mention of `execute_code_intercept` inside a preceding `/* */` block \
         comment must not be mistaken for the shorthand field-init form — the field's \
         REAL assignment (`execute_code_intercept: None`) further down the span must \
         still be found and correctly classified as unset"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Phase 57 Plan 16 (SC-1 backstop, D-08, D-13, D-15, Pitfall 14): structural
// sweeps that keep every security surface wired.
//
// Each check is a pure function over `(label, content)`, so it is proven BOTH
// against the real tree (the `#[test]`s named in the plan) and against synthetic
// content that violates it (the `synthetic_*` tests): a check that cannot fail is
// theatre. Every check has a behavioural counterpart in plans 07-14 and 17; the
// sweep only makes the wiring hard to undo silently.
// ─────────────────────────────────────────────────────────────────────────────

/// A host that must boot the detection sidecar (SC-1, D-02, D-21).
struct BootSpec {
    /// Workspace-relative path of the host's entry file.
    path: &'static str,
    /// The boot function that file must define AND call.
    boot_fn: &'static str,
    /// Text that must appear (outside comments) inside `boot_fn`'s body.
    body_needle: &'static str,
}

/// The agent hosts after plans 10, 12 and 13: the CLI (also hosts cron and kanban
/// workers), the web UI server, and the gateway runner (also hosts the cron, kanban
/// and notifier schedulers). The web host boots through `boot_security_with`, which
/// the second entry pins to the real `service::boot` call.
const BOOT_SPECS: &[BootSpec] = &[
    BootSpec {
        path: "crates/ironhermes-cli/src/main.rs",
        boot_fn: "boot_security_for_cli",
        body_needle: "boot(",
    },
    BootSpec {
        path: "crates/iron_hermes_ui/src/main.rs",
        boot_fn: "boot_security_for_web",
        body_needle: "boot_security_with(",
    },
    BootSpec {
        path: "crates/iron_hermes_ui/src/main.rs",
        boot_fn: "boot_security_with",
        body_needle: "ironhermes_security::service::boot(",
    },
    BootSpec {
        path: "crates/ironhermes-gateway/src/runner.rs",
        boot_fn: "boot_security",
        body_needle: "ironhermes_security::service::boot(",
    },
];

/// Production `ToolRegistry::new()` / `::default()` sites that are deliberately NOT
/// screened: `(workspace-relative path, enclosing fn or "*" for a whole file, reason)`.
/// An entry with an empty reason fails, and so does an entry that matches no site (a
/// stale exemption would quietly pre-approve a future registry in that fn).
const REGISTRY_SCREEN_EXEMPT: &[(&str, &str, &str)] = &[
    (
        "crates/ironhermes-agent/src/agent_loop.rs",
        "for_tests",
        "`#[cfg(any(test, feature = \"test-support\"))]` constructor; the cfg(test) region helper does not recognise the `any(..)` form",
    ),
    (
        "crates/ironhermes-agent/src/agent_runtime.rs",
        "for_tests_inner",
        "`#[cfg(any(test, feature = \"test-support\"))]` constructor; the cfg(test) region helper does not recognise the `any(..)` form",
    ),
    (
        "crates/ironhermes-cli/src/tui_rata/app.rs",
        "test_deps",
        "`#[cfg(feature = \"test-support\")]` helper, compiled only into test builds",
    ),
    (
        "crates/ironhermes-cli/src/doctor.rs",
        "run_doctor_check",
        "inspection only: lists tool availability for `ironhermes doctor`, never dispatches a call",
    ),
    (
        "crates/ironhermes-cli/src/setup.rs",
        "build_full_registry",
        "inspection only: the setup wizard lists tools, never dispatches a call",
    ),
    (
        "crates/ironhermes-cli/src/toolset_cmd.rs",
        "cmd_toolset_list",
        "inspection only: tool availability listing, never dispatch",
    ),
    (
        "crates/ironhermes-cli/src/toolset_cmd.rs",
        "cmd_toolset_show",
        "inspection only: tool availability listing, never dispatch",
    ),
    (
        "crates/ironhermes-cli/src/mcp_config.rs",
        "cmd_connect",
        "OAuth paste-back MCP manager: connects a server for credential exchange, runs no agent turn",
    ),
    (
        "crates/ironhermes-cli/src/cron.rs",
        "build_cron_runner_ctx",
        "empty registry, so no tool can be called; the CLI cron host still boots the sidecar (plan 13) and every prompt is re-screened at run time (plan 11)",
    ),
];

/// `execute_gated_command` call sites whose policy argument is a typed `GatePolicy`
/// function parameter rather than a `for_session` binding: `(path, fn, reason)`. The
/// caller owns the choice, so the exemption names why that is acceptable.
const POLICY_PASSTHROUGH_EXEMPT: &[(&str, &str, &str)] = &[(
    "crates/ironhermes-gateway/src/handler.rs",
    "handle_shell_exec",
    "typed `GatePolicy` parameter; documented to receive `GatePolicy::for_session(session_id)` and has no production caller outside tests",
)];

/// Files that DEFINE `install_fake_for_tests`; they necessarily contain the name.
const FAKE_BACKEND_DEFINING_FILES: &[(&str, &str)] = &[(
    "crates/ironhermes-security/src/testing.rs",
    "the module that defines the in-process fake (always compiled so integration tests need no feature flag; hidden from docs)",
)];

/// Production `scan_cron_prompt(` call sites (the regex-only creation-time scan) that are
/// allowed to stay unscreened: `(path, exact count allowed, reason)`. Every job prompt is
/// re-screened at run time by `scan_cron_prompt_screened` (plan 11), so these creation paths
/// only miss the early sidecar verdict; they publish no `JobBlocked` event (SECURITY.md,
/// "Out-of-process job events"). A site beyond the listed count fails the sweep.
const BARE_CRON_SCAN_ALLOWED: &[(&str, usize, &str)] = &[
    (
        "crates/ironhermes-cli/src/cron.rs",
        2,
        "CLI `cron create` / `cron edit` keep the regex-only creation-time scan; run time re-screens (plan 11)",
    ),
    (
        "crates/ironhermes-cron/src/writer_impl.rs",
        2,
        "cron-writer create / update keep the regex-only creation-time scan; run time re-screens (plan 11)",
    ),
];

// ── Source-scanning helpers for the plan 16 sweeps ───────────────────────────
//
// These are stricter than `comment_ranges` / `find_matching_close` above (which the older
// sweeps rely on and which stay as they are): string and char literals are skipped, so a
// glob such as `"**/*.rs"` cannot open a phantom block comment and a `"{"` in a format
// string cannot unbalance a brace scan. A `#[cfg(test)]` that only appears inside a comment
// is not treated as a test region (it would otherwise hide the production code after it).

/// End index (exclusive) of the comment starting at `i`, if one starts there.
fn skip_comment(content: &str, i: usize) -> Option<usize> {
    let bytes = content.as_bytes();
    if bytes[i] != b'/' {
        return None;
    }
    match bytes.get(i + 1) {
        Some(b'/') => Some(content[i..].find('\n').map_or(content.len(), |p| i + p)),
        Some(b'*') => {
            let (mut depth, mut j) = (1i32, i + 2);
            while j < bytes.len() && depth > 0 {
                if bytes[j] == b'/' && bytes.get(j + 1) == Some(&b'*') {
                    depth += 1;
                    j += 2;
                } else if bytes[j] == b'*' && bytes.get(j + 1) == Some(&b'/') {
                    depth -= 1;
                    j += 2;
                } else {
                    j += 1;
                }
            }
            Some(j)
        }
        _ => None,
    }
}

/// End index (exclusive) of the string, raw string or char literal starting at `i`, if one
/// starts there. A lifetime (`'a`) is not a literal.
fn skip_literal(content: &str, i: usize) -> Option<usize> {
    let bytes = content.as_bytes();
    match bytes[i] {
        b'"' => {
            let mut j = i + 1;
            while j < bytes.len() {
                match bytes[j] {
                    b'\\' => j += 2,
                    b'"' => return Some(j + 1),
                    _ => j += 1,
                }
            }
            Some(bytes.len())
        }
        b'r' => {
            let prev_ok = i == 0
                || !is_ident_char(bytes[i - 1])
                || (bytes[i - 1] == b'b' && (i == 1 || !is_ident_char(bytes[i - 2])));
            if !prev_ok {
                return None;
            }
            let hashes = bytes[i + 1..].iter().take_while(|b| **b == b'#').count();
            if bytes.get(i + 1 + hashes) != Some(&b'"') {
                return None;
            }
            let body_start = i + 2 + hashes;
            let closing = format!("\"{}", "#".repeat(hashes));
            Some(
                content[body_start..]
                    .find(&closing)
                    .map_or(bytes.len(), |p| body_start + p + closing.len()),
            )
        }
        b'\'' => {
            let mut chars = content[i + 1..].chars();
            match chars.next() {
                Some('\\') => {
                    let mut j = i + 3;
                    while j < bytes.len() && bytes[j] != b'\'' {
                        j += 1;
                    }
                    Some((j + 1).min(bytes.len()))
                }
                Some(c) if chars.next() == Some('\'') => Some(i + 1 + c.len_utf8() + 1),
                _ => None,
            }
        }
        _ => None,
    }
}

/// Every comment span in `content`, ignoring comment markers inside literals.
fn comment_spans(content: &str) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut i = 0usize;
    while i < content.len() {
        if let Some(end) = skip_comment(content, i) {
            spans.push((i, end));
            i = end.max(i + 1);
        } else if let Some(end) = skip_literal(content, i) {
            i = end.max(i + 1);
        } else {
            i += 1;
        }
    }
    spans
}

/// Index of the delimiter closing the `(`, `[` or `{` at `open_idx`, skipping literals and
/// comments.
fn matching_close(content: &str, open_idx: usize) -> Option<usize> {
    let bytes = content.as_bytes();
    let (mut depth, mut i) = (0i32, open_idx);
    while i < bytes.len() {
        if let Some(end) = skip_comment(content, i).or_else(|| skip_literal(content, i)) {
            i = end.max(i + 1);
            continue;
        }
        match bytes[i] {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Split an argument list on its top-level commas.
fn split_top_level(args: &str) -> Vec<&str> {
    let bytes = args.as_bytes();
    let (mut depth, mut start, mut i) = (0i32, 0usize, 0usize);
    let mut out = Vec::new();
    while i < bytes.len() {
        if let Some(end) = skip_comment(args, i).or_else(|| skip_literal(args, i)) {
            i = end.max(i + 1);
            continue;
        }
        match bytes[i] {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth -= 1,
            b',' if depth == 0 => {
                out.push(&args[start..i]);
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    if !args[start..].trim().is_empty() {
        out.push(&args[start..]);
    }
    out
}

/// `true` when the `cfg` predicate is only true in a test build: `test`, or an `all(..)`
/// with `test` as a direct argument. `any(test, feature = "x")` and `not(..)` are not.
fn cfg_requires_test(pred: &str) -> bool {
    let pred = pred.trim();
    if pred == "test" {
        return true;
    }
    pred.strip_prefix("all(")
        .and_then(|s| s.strip_suffix(')'))
        .is_some_and(|inner| split_top_level(inner).into_iter().any(cfg_requires_test))
}

/// End (exclusive) of the item starting at `start`: its first top-level `{ .. }` block or its
/// first top-level `;`.
fn item_end(content: &str, start: usize) -> usize {
    let bytes = content.as_bytes();
    let (mut depth, mut j) = (0i32, start);
    while j < bytes.len() {
        if let Some(end) = skip_comment(content, j).or_else(|| skip_literal(content, j)) {
            j = end.max(j + 1);
            continue;
        }
        match bytes[j] {
            b'(' | b'[' => depth += 1,
            b')' | b']' => depth -= 1,
            b'{' if depth == 0 => {
                return matching_close(content, j).map_or(bytes.len(), |c| c + 1);
            }
            b';' if depth == 0 => return j + 1,
            _ => {}
        }
        j += 1;
    }
    bytes.len()
}

/// Byte ranges gated by a `#[cfg(..)]` that only holds in a test build, with literals and
/// comments skipped (the older [`cfg_test_regions`] is blind to braces inside strings, which
/// ends a region early in files with brace-bearing test fixtures such as `registry.rs`).
fn strict_test_regions(content: &str, comments: &[(usize, usize)]) -> Vec<(usize, usize)> {
    let in_comment = |pos: usize| comments.iter().any(|(s, e)| pos >= *s && pos < *e);
    if content
        .match_indices("#![cfg(test)]")
        .any(|(p, _)| !in_comment(p))
    {
        return vec![(0, content.len())];
    }
    let bytes = content.as_bytes();
    let mut regions = Vec::new();
    let mut from = 0usize;
    while let Some(rel) = content[from..].find("#[cfg(") {
        let attr = from + rel;
        from = attr + 1;
        if in_comment(attr) {
            continue;
        }
        let paren = attr + "#[cfg".len();
        let Some(close) = matching_close(content, paren) else {
            continue;
        };
        if !cfg_requires_test(&content[paren + 1..close]) {
            continue;
        }
        // Past the attribute's closing `]`, then any further stacked attributes, to the item.
        let mut i = content[close + 1..]
            .find(']')
            .map_or(close + 1, |p| close + 1 + p + 1);
        loop {
            while i < bytes.len() && (bytes[i] as char).is_whitespace() {
                i += 1;
            }
            if content[i..].starts_with("#[")
                && let Some(end) = matching_close(content, i + 1)
            {
                i = end + 1;
                continue;
            }
            break;
        }
        let end = item_end(content, i);
        regions.push((attr, end));
        from = end.max(from);
    }
    regions
}

/// Whole files that are `#[cfg(test)] mod <name>;` modules declared in another file:
/// `(path, declaring file, module name, reason)`. Their text is test code even though no
/// attribute appears in the file itself. [`declared_test_modules_are_cfg_test`] checks the
/// declaration, so an entry cannot outlive the attribute that justifies it.
const TEST_MODULE_FILES: &[(&str, &str, &str, &str)] = &[(
    "crates/ironhermes-cli/src/batch/tests.rs",
    "crates/ironhermes-cli/src/batch/mod.rs",
    "tests",
    "the batch runner's unit tests, declared `#[cfg(test)] mod tests;`",
)];

/// One file with its comment spans and test-only regions.
struct Src<'a> {
    content: &'a str,
    comments: Vec<(usize, usize)>,
    tests: Vec<(usize, usize)>,
}

impl<'a> Src<'a> {
    /// A file known by its workspace-relative `label`; a [`TEST_MODULE_FILES`] entry is all test.
    fn for_label(label: &str, content: &'a str) -> Self {
        let mut src = Self::new(content);
        if TEST_MODULE_FILES.iter().any(|(p, _, _, _)| *p == label) {
            src.tests = vec![(0, content.len())];
        }
        src
    }

    fn new(content: &'a str) -> Self {
        let comments = comment_spans(content);
        let tests = strict_test_regions(content, &comments);
        Self {
            content,
            comments,
            tests,
        }
    }

    fn in_comment(&self, pos: usize) -> bool {
        self.comments.iter().any(|(s, e)| pos >= *s && pos < *e)
    }

    fn in_test(&self, pos: usize) -> bool {
        self.tests.iter().any(|(s, e)| pos >= *s && pos < *e)
    }

    fn is_prod(&self, pos: usize) -> bool {
        !self.in_comment(pos) && !self.in_test(pos)
    }

    /// 1-based line of byte offset `pos`.
    fn line(&self, pos: usize) -> usize {
        self.content[..pos].matches('\n').count() + 1
    }

    /// Offsets of `needle` outside comments and `#[cfg(test)]` regions.
    fn prod_find_all(&self, needle: &str) -> Vec<usize> {
        self.content
            .match_indices(needle)
            .map(|(p, _)| p)
            .filter(|p| self.is_prod(*p))
            .collect()
    }

    /// Like [`Self::prod_find_all`] but the match must start at an identifier boundary.
    fn prod_find_idents(&self, needle: &str) -> Vec<usize> {
        let bytes = self.content.as_bytes();
        self.prod_find_all(needle)
            .into_iter()
            .filter(|p| *p == 0 || !is_ident_char(bytes[*p - 1]))
            .collect()
    }

    /// `true` when the text before `pos` ends with `fn ` (the match is a definition).
    fn is_definition(&self, pos: usize) -> bool {
        self.content[..pos].ends_with("fn ")
    }
}

/// A function: its name, the offset of its `fn` keyword, and its opening body brace.
struct FnInfo {
    name: String,
    kw_start: usize,
    body_start: usize,
}

/// Body braces of the fn whose name ends at `after_name`, or `None` for a body-less fn.
fn fn_body_range(content: &str, after_name: usize) -> Option<(usize, usize)> {
    let bytes = content.as_bytes();
    let (mut depth, mut j) = (0i32, after_name);
    while j < bytes.len() {
        if let Some(end) = skip_comment(content, j).or_else(|| skip_literal(content, j)) {
            j = end.max(j + 1);
            continue;
        }
        match bytes[j] {
            b'(' | b'[' => depth += 1,
            b')' | b']' => depth -= 1,
            b'{' if depth == 0 => return matching_close(content, j).map(|c| (j, c)),
            b';' if depth == 0 => return None,
            _ => {}
        }
        j += 1;
    }
    None
}

/// The innermost fn whose body contains `pos`.
fn enclosing_fn(src: &Src, pos: usize) -> Option<FnInfo> {
    let content = src.content;
    let bytes = content.as_bytes();
    let mut search_end = pos;
    while let Some(k) = content[..search_end].rfind("fn ") {
        search_end = k;
        if src.in_comment(k) || (k > 0 && is_ident_char(bytes[k - 1])) {
            continue;
        }
        let name_start = k + 3;
        let name_len = bytes[name_start..]
            .iter()
            .take_while(|b| is_ident_char(**b))
            .count();
        if name_len == 0 {
            continue;
        }
        let Some((open, close)) = fn_body_range(content, name_start + name_len) else {
            continue;
        };
        if open < pos && pos < close {
            return Some(FnInfo {
                name: content[name_start..name_start + name_len].to_string(),
                kw_start: k,
                body_start: open,
            });
        }
    }
    None
}

/// Collapse whitespace runs so a multi-line argument reads on one line in a message.
fn compact(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `Ok` when the `policy` argument of the gated call at `call_pos` is `GatePolicy::for_session(..)`,
/// directly, through a `let` binding in the same fn, or (listed) as a typed fn parameter.
fn policy_arg_status(
    src: &Src,
    call_pos: usize,
    policy: &str,
    label: &str,
    enclosing: Option<&FnInfo>,
) -> Result<(), String> {
    if policy.contains("GatePolicy::for_session(") {
        return Ok(());
    }
    let is_ident = !policy.is_empty()
        && policy.bytes().all(is_ident_char)
        && !policy.as_bytes()[0].is_ascii_digit()
        && policy != "true"
        && policy != "false";
    if !is_ident {
        return Err(format!(
            "policy argument `{}` is not GatePolicy::for_session(..)",
            compact(policy)
        ));
    }
    let Some(f) = enclosing else {
        return Err(format!("policy `{policy}` is not bound inside any fn"));
    };
    let before_call = &src.content[f.body_start..call_pos];
    for pat in [format!("let {policy} ="), format!("let mut {policy} =")] {
        if let Some(at) = before_call.rfind(&pat) {
            let init_start = f.body_start + at + pat.len();
            let init_end = src.content[init_start..]
                .find(';')
                .map_or(call_pos, |p| init_start + p);
            return if src.content[init_start..init_end].contains("GatePolicy::for_session(") {
                Ok(())
            } else {
                Err(format!(
                    "`{policy}` is bound in fn {} from something other than GatePolicy::for_session(..)",
                    f.name
                ))
            };
        }
    }
    let sig = &src.content[f.kw_start..f.body_start];
    let param = format!("{policy}:");
    let sig_bytes = sig.as_bytes();
    if let Some(at) = sig
        .match_indices(&param)
        .map(|(p, _)| p)
        .find(|p| *p == 0 || !is_ident_char(sig_bytes[*p - 1]))
    {
        let ty_end = sig[at..].find([',', ')']).map_or(sig.len(), |p| at + p);
        if sig[at..ty_end].contains("GatePolicy") {
            return if POLICY_PASSTHROUGH_EXEMPT.iter().any(|(p, name, reason)| {
                *p == label && *name == f.name && !reason.trim().is_empty()
            }) {
                Ok(())
            } else {
                Err(format!(
                    "`{policy}` is a typed GatePolicy parameter of fn {} that is not listed in \
                     POLICY_PASSTHROUGH_EXEMPT",
                    f.name
                ))
            };
        }
    }
    Err(format!(
        "`{policy}` is not bound from GatePolicy::for_session(..) in fn {}",
        f.name
    ))
}

/// SC-1, D-09, D-21: every production `execute_gated_command(` call must judge the call
/// under the live session level. Returns `(sites seen, failures)`.
fn gated_call_report(label: &str, content: &str) -> (usize, Vec<String>) {
    const NEEDLE: &str = "execute_gated_command(";
    let src = Src::for_label(label, content);
    let (mut sites, mut failures) = (0usize, Vec::new());
    for pos in src.prod_find_all(NEEDLE) {
        if src.is_definition(pos) {
            continue;
        }
        sites += 1;
        let line = src.line(pos);
        let enclosing = enclosing_fn(&src, pos);
        let fn_name = enclosing.as_ref().map_or("<none>", |f| f.name.as_str());
        let open = pos + NEEDLE.len() - 1;
        let Some(close) = matching_close(content, open) else {
            failures.push(format!("{label}:{line} (fn {fn_name}): unbalanced call"));
            continue;
        };
        let args = split_top_level(&content[open + 1..close]);
        // execute_gated_command(tool, classify_arg, guard, gate, audit, session, surface,
        // chat_id, policy, is_remote, forward_env, run)
        let Some(policy) = args.get(8).map(|a| a.trim()) else {
            failures.push(format!(
                "{label}:{line} (fn {fn_name}): cannot find the policy argument ({} arguments)",
                args.len()
            ));
            continue;
        };
        if let Err(why) = policy_arg_status(&src, pos, policy, label, enclosing.as_ref()) {
            failures.push(format!("{label}:{line} (fn {fn_name}): {why}"));
        }
    }
    (sites, failures)
}

/// SC-1, D-02, D-21: the host defines `spec.boot_fn`, its body calls the real boot, and the
/// host actually calls the function. Returns failures.
fn host_boot_failures(label: &str, content: &str, spec: &BootSpec) -> Vec<String> {
    let src = Src::for_label(label, content);
    let name = spec.boot_fn;
    let call_needle = format!("{name}(");
    let occurrences = src.prod_find_idents(&call_needle);
    let Some(def) = occurrences.iter().copied().find(|p| src.is_definition(*p)) else {
        return vec![format!(
            "{label}: no production `fn {name}(` — the host no longer defines its sidecar boot (SC-1, D-02)"
        )];
    };
    let mut failures = Vec::new();
    match fn_body_range(content, def + name.len()) {
        Some((open, close)) => {
            let calls_real_boot = src
                .prod_find_all(spec.body_needle)
                .into_iter()
                .any(|p| p > open && p < close);
            if !calls_real_boot {
                failures.push(format!(
                    "{label}: `fn {name}` no longer contains `{}` — it boots nothing (SC-1, D-02)",
                    spec.body_needle
                ));
            }
        }
        None => failures.push(format!("{label}: `fn {name}` has no body")),
    }
    if !occurrences.iter().any(|p| !src.is_definition(*p)) {
        failures.push(format!(
            "{label}: `{name}(` is defined but never called outside comments and tests — the host \
             starts without the sidecar (SC-1, D-02, D-21)"
        ));
    }
    failures
}

/// D-08: a production reader of the deprecated `autonomous.yolo` boolean.
fn yolo_reader_failures(label: &str, content: &str) -> Vec<String> {
    let src = Src::for_label(label, content);
    let bytes = content.as_bytes();
    let mut failures = Vec::new();
    for (pos, _) in content.match_indices("autonomous") {
        if (pos > 0 && is_ident_char(bytes[pos - 1])) || !src.is_prod(pos) {
            continue;
        }
        let mut j = pos + "autonomous".len();
        while j < bytes.len() && (bytes[j] as char).is_whitespace() {
            j += 1;
        }
        if bytes.get(j) != Some(&b'.') {
            continue;
        }
        j += 1;
        while j < bytes.len() && (bytes[j] as char).is_whitespace() {
            j += 1;
        }
        if content[j..].starts_with("yolo") && bytes.get(j + 4).is_none_or(|b| !is_ident_char(*b)) {
            failures.push(format!("{label}:{}", src.line(pos)));
        }
    }
    failures
}

/// SC-1, T-57-74: a production `ToolRegistry` construction that is neither screened nor
/// exempt. Returns `(failures, indices into `exempt` that matched a site, sites seen)`.
fn registry_failures(
    label: &str,
    content: &str,
    exempt: &[(&str, &str, &str)],
) -> (Vec<String>, Vec<usize>, usize) {
    let src = Src::for_label(label, content);
    let mut positions: Vec<usize> = ["ToolRegistry::new()", "ToolRegistry::default()"]
        .iter()
        .flat_map(|n| src.prod_find_all(n))
        .collect();
    positions.sort_unstable();
    let screens = src
        .prod_find_all("set_tool_screen(")
        .into_iter()
        .filter(|p| !src.is_definition(*p))
        .count();
    let mut used = std::collections::BTreeSet::new();
    let mut unscreened = Vec::new();
    for pos in &positions {
        let fn_name = enclosing_fn(&src, *pos).map_or_else(|| "<module>".to_string(), |f| f.name);
        if let Some(i) = exempt
            .iter()
            .position(|(p, f, _)| *p == label && (*f == "*" || *f == fn_name))
        {
            used.insert(i);
            continue;
        }
        unscreened.push(format!("{label}:{} (fn {fn_name})", src.line(*pos)));
    }
    let mut failures = Vec::new();
    if screens < unscreened.len() {
        failures.push(format!(
            "{} unscreened production ToolRegistry site(s) but {screens} set_tool_screen( call(s) in the file: {}",
            unscreened.len(),
            unscreened.join(", ")
        ));
    }
    (failures, used.into_iter().collect(), positions.len())
}

/// D-15: `install_fake_for_tests` outside test code.
fn fake_backend_failures(label: &str, content: &str) -> Vec<String> {
    if FAKE_BACKEND_DEFINING_FILES.iter().any(|(p, _)| *p == label) {
        return Vec::new();
    }
    let src = Src::for_label(label, content);
    src.prod_find_idents("install_fake_for_tests")
        .into_iter()
        .map(|p| format!("{label}:{}", src.line(p)))
        .collect()
}

/// D-13: an OS sandbox spawned from production code.
fn os_sandbox_failures(label: &str, content: &str) -> Vec<String> {
    const NEEDLE: &str = "Command::new(";
    let src = Src::for_label(label, content);
    let bytes = content.as_bytes();
    let mut failures = Vec::new();
    for pos in src.prod_find_all(NEEDLE) {
        let mut j = pos + NEEDLE.len();
        while j < bytes.len() && (bytes[j] as char).is_whitespace() {
            j += 1;
        }
        if bytes.get(j) != Some(&b'"') {
            continue;
        }
        let Some(end) = skip_literal(content, j) else {
            continue;
        };
        let program = &content[j + 1..end.saturating_sub(1).max(j + 1)];
        let base = program.rsplit('/').next().unwrap_or(program);
        if base == "sandbox-exec" || base == "bwrap" {
            failures.push(format!("{label}:{} spawns `{program}`", src.line(pos)));
        }
    }
    failures
}

/// Production bare `scan_cron_prompt(` call sites in this file. Returns `(count, failures)`;
/// a file may hold at most its [`BARE_CRON_SCAN_ALLOWED`] count.
fn bare_cron_scan_failures(label: &str, content: &str) -> (usize, Vec<String>) {
    let src = Src::for_label(label, content);
    let count = src
        .prod_find_idents("scan_cron_prompt(")
        .into_iter()
        .filter(|p| !src.is_definition(*p))
        .count();
    let allowed = BARE_CRON_SCAN_ALLOWED
        .iter()
        .find(|(p, _, reason)| *p == label && !reason.trim().is_empty())
        .map_or(0, |(_, n, _)| *n);
    let failures = if count > allowed {
        vec![format!(
            "{label}: {count} bare scan_cron_prompt( call(s), at most {allowed} allowed — use \
             scan_cron_prompt_screened, or list the site in BARE_CRON_SCAN_ALLOWED with a reason"
        )]
    } else {
        Vec::new()
    };
    (count, failures)
}

fn join_failures(failures: Vec<String>) -> String {
    failures.join("\n")
}

/// SC-1 (D-09): every gated call passes `GatePolicy::for_session(..)`, directly or through a
/// binding in the same fn, so a mid-session `/yolo <N>` reaches the next call.
#[test]
fn every_gated_call_uses_session_policy() {
    let mut sites = 0usize;
    let mut failures = Vec::new();
    for (label, content) in swept_sources() {
        let (n, f) = gated_call_report(&label, &content);
        sites += n;
        failures.extend(f);
    }
    assert!(
        sites >= 13,
        "expected at least the 13 known production execute_gated_command sites (gateway 5 incl. \
         handle_shell_exec, UI server 2, TUI 2, CLI 2, ACP 2) — found {sites}. The probe's \
         matching broke and would otherwise pass vacuously."
    );
    assert!(
        failures.is_empty(),
        "SC-1/D-09: production execute_gated_command call(s) that do not pass \
         GatePolicy::for_session(..) — a bare bool or fixed level cannot follow `/yolo <N>`:\n{}",
        join_failures(failures)
    );
}

/// SC-1 (D-02, D-21): every agent host boots the sidecar, otherwise every screen reads
/// "sidecar unavailable" and the level-3 default blocks Warn-class tools.
#[test]
fn every_agent_host_boots_sidecar() {
    let sources = swept_sources();
    let mut failures = Vec::new();
    for spec in BOOT_SPECS {
        match sources.iter().find(|(l, _)| l == spec.path) {
            Some((label, content)) => failures.extend(host_boot_failures(label, content, spec)),
            None => failures.push(format!("{}: host file not found by the sweep", spec.path)),
        }
    }
    assert!(
        failures.is_empty(),
        "SC-1/D-02/D-21: an agent host no longer boots the security sidecar:\n{}",
        join_failures(failures)
    );
}

/// D-08: the deprecated `autonomous.yolo` boolean is read only by the level resolution in
/// config.rs; any other reader reintroduces a captured bool that cannot follow `/yolo <N>`.
#[test]
fn no_yolo_boolean_readers() {
    let mut files_seen = 0usize;
    let mut failures = Vec::new();
    for (label, content) in swept_sources() {
        files_seen += 1;
        if label == "crates/ironhermes-core/src/config.rs" {
            continue;
        }
        failures.extend(yolo_reader_failures(&label, &content));
    }
    assert!(
        files_seen > 50,
        "the sweep walked only {files_seen} files — the walk is broken"
    );
    assert!(
        failures.is_empty(),
        "D-08: production code outside config.rs reads `autonomous.yolo` — read \
         `Config::effective_defcon_level()` / the session registry instead:\n{}",
        join_failures(failures)
    );
}

/// SC-1 (T-57-74): every production `ToolRegistry` is screened or carries a reasoned exemption.
#[test]
fn every_production_registry_is_screened() {
    for (path, fn_name, reason) in REGISTRY_SCREEN_EXEMPT {
        assert!(
            !reason.trim().is_empty(),
            "T-57-74: the registry exemption ({path}, {fn_name}) has no reason"
        );
    }
    let mut failures = Vec::new();
    let mut used = vec![false; REGISTRY_SCREEN_EXEMPT.len()];
    let mut sites = 0usize;
    for (label, content) in swept_sources() {
        let (f, matched, n) = registry_failures(&label, &content, REGISTRY_SCREEN_EXEMPT);
        failures.extend(f);
        sites += n;
        for i in matched {
            used[i] = true;
        }
    }
    assert!(
        sites >= 8,
        "expected at least 8 production ToolRegistry construction sites (screened + exempt) — \
         found {sites}; the probe's matching broke and would otherwise pass vacuously"
    );
    let stale: Vec<String> = REGISTRY_SCREEN_EXEMPT
        .iter()
        .zip(&used)
        .filter(|(_, u)| !**u)
        .map(|((p, f, _), _)| format!("{p} :: {f}"))
        .collect();
    assert!(
        stale.is_empty(),
        "T-57-74: registry exemption(s) match no production site (stale — remove them): {stale:?}"
    );
    assert!(
        failures.is_empty(),
        "SC-1/T-57-74: production ToolRegistry construction site(s) with no `set_tool_screen(` \
         in the file and no reasoned exemption — an unscreened registry lets every tool call \
         skip the DEFCON screen:\n{}",
        join_failures(failures)
    );
}

/// D-15: the scripted fake backend is a test fixture; production code must never install it.
#[test]
fn fake_backend_only_in_tests() {
    let mut failures = Vec::new();
    for (label, content) in swept_sources() {
        failures.extend(fake_backend_failures(&label, &content));
    }
    assert!(
        failures.is_empty(),
        "D-15: `install_fake_for_tests` appears in production code (a fake would replace the \
         real sidecar verdicts):\n{}",
        join_failures(failures)
    );
}

/// D-13: the `execute_code` OS sandbox is the successor phase; none ships in Phase 57.
#[test]
fn no_os_sandbox_built_in_phase_57() {
    let mut failures = Vec::new();
    for (label, content) in swept_sources() {
        failures.extend(os_sandbox_failures(&label, &content));
    }
    assert!(
        failures.is_empty(),
        "D-13: production code spawns an OS sandbox (sandbox-exec / bwrap); the sandbox is a \
         named successor phase, not part of Phase 57:\n{}",
        join_failures(failures)
    );
}

/// Plan 11 hand-off (57-11): the regex-only creation-time `scan_cron_prompt(` is allowed at
/// the four listed CLI / cron-writer sites only; every other production path must call
/// `scan_cron_prompt_screened` so the sidecar sees the prompt.
#[test]
fn bare_cron_scan_sites_are_allow_listed() {
    let (mut listed_seen, mut failures) = (0usize, Vec::new());
    for (label, content) in swept_sources() {
        let (count, f) = bare_cron_scan_failures(&label, &content);
        if BARE_CRON_SCAN_ALLOWED.iter().any(|(p, _, _)| *p == label) {
            listed_seen += count;
        }
        failures.extend(f);
    }
    let listed_total: usize = BARE_CRON_SCAN_ALLOWED.iter().map(|(_, n, _)| *n).sum();
    assert_eq!(
        listed_seen, listed_total,
        "the allow-list is stale: it names {listed_total} bare scan_cron_prompt( site(s) but the \
         tree has {listed_seen} — update BARE_CRON_SCAN_ALLOWED (and SECURITY.md accepted risk 9)"
    );
    assert!(
        failures.is_empty(),
        "57-11: production bare scan_cron_prompt( call(s) outside the allow-list — the sidecar \
         never sees these prompts at creation time:\n{}",
        join_failures(failures)
    );
}

/// A whole-file test exemption is only honest while its declaring file still gates the
/// module with `#[cfg(test)]`; otherwise the file would ship in production unswept.
#[test]
fn declared_test_modules_are_cfg_test() {
    for (path, decl, module, reason) in TEST_MODULE_FILES {
        assert!(
            !reason.trim().is_empty(),
            "{path}: test-module entry has no reason"
        );
        let file = workspace_root().join(path);
        assert!(
            file.is_file(),
            "{path}: test-module file no longer exists — remove the entry"
        );
        let declaration = std::fs::read_to_string(workspace_root().join(decl))
            .unwrap_or_else(|e| panic!("{decl}: cannot read the declaring file: {e}"));
        let gated = declaration.match_indices("#[cfg(test)]").any(|(p, _)| {
            declaration[p + "#[cfg(test)]".len()..]
                .trim_start()
                .starts_with(&format!("mod {module};"))
        });
        assert!(
            gated,
            "{decl} does not declare `#[cfg(test)] mod {module};` — {path} would be production code"
        );
    }
}

// ── Each check must be able to fail: synthetic violating content ─────────────

#[test]
fn synthetic_cfg_test_regions_follow_the_predicate() {
    let src = "#[cfg(all(test, feature = \"server\"))]\nmod a { fn f() { let r = ToolRegistry::new(); } }\n";
    assert_eq!(
        registry_failures("crates/x/src/a.rs", src, &[]).2,
        0,
        "all(test, ..) is test-only"
    );
    let any = "#[cfg(any(test, feature = \"test-support\"))]\nfn for_tests() { let r = ToolRegistry::new(); }\n";
    assert_eq!(
        registry_failures("crates/x/src/a.rs", any, &[]).2,
        1,
        "any(test, feature) ships in a feature build"
    );
    let not_test = "#[cfg(not(test))]\nfn prod() { let r = ToolRegistry::new(); }\n";
    assert_eq!(registry_failures("crates/x/src/a.rs", not_test, &[]).2, 1);
    let braces = "#[cfg(test)]\nmod t { fn f() { let s = \"}}\"; let r = ToolRegistry::new(); } }\nfn prod() { let r = ToolRegistry::default(); }\n";
    let (_, _, n) = registry_failures("crates/x/src/a.rs", braces, &[]);
    assert_eq!(
        n, 1,
        "a brace inside a string must not end the test region early"
    );
    let inner = "#![cfg(test)]\nfn f() { let r = ToolRegistry::new(); }\n";
    assert_eq!(
        registry_failures("crates/x/src/a.rs", inner, &[]).2,
        0,
        "an inner cfg(test) gates the file"
    );
}

#[test]
fn synthetic_unlisted_bare_cron_scan_is_flagged() {
    let bad = "fn create() { let _ = scan_cron_prompt(&p); }\n";
    let (n, f) = bare_cron_scan_failures("crates/x/src/a.rs", bad);
    assert_eq!(
        (n, f.len()),
        (1, 1),
        "an unlisted bare scan must be flagged: {f:?}"
    );
    let screened = "fn create() { let _ = scan_cron_prompt_screened(&p, lvl, \"j\"); }\n";
    assert_eq!(
        bare_cron_scan_failures("crates/x/src/a.rs", screened),
        (0, vec![])
    );
    let def = "pub fn scan_cron_prompt(p: &str) -> Result<(), String> { Ok(()) }\n";
    assert_eq!(
        bare_cron_scan_failures("crates/x/src/a.rs", def).0,
        0,
        "a definition is not a call"
    );
    let in_test = "#[cfg(test)]\nmod t { fn f() { scan_cron_prompt(\"x\"); } }\n";
    assert_eq!(bare_cron_scan_failures("crates/x/src/a.rs", in_test).0, 0);
    let two = "fn a() { scan_cron_prompt(&p); }\nfn b() { scan_cron_prompt(&q); }\nfn c() { scan_cron_prompt(&r); }\n";
    let (n, f) = bare_cron_scan_failures("crates/ironhermes-cli/src/cron.rs", two);
    assert_eq!(
        (n, f.len()),
        (3, 1),
        "a third site beyond the two listed must be flagged"
    );
}

#[test]
fn synthetic_literals_and_comments_do_not_hide_or_invent_sites() {
    // A glob in a string must not open a block comment that hides the real call after it.
    let glob = "fn f() { let g = \"src/**/*.rs\"; let _ = install_fake_for_tests(x); }\n";
    assert_eq!(fake_backend_failures("crates/x/src/a.rs", glob).len(), 1);
    // A cfg(test) mention inside a comment must not hide the production code after it.
    let doc = "// not #[cfg(test)] code\nfn f() { let _ = install_fake_for_tests(x); }\n";
    assert_eq!(fake_backend_failures("crates/x/src/a.rs", doc).len(), 1);
    // A brace inside a string must not unbalance the enclosing fn.
    let brace = "fn outer() { let s = \"{\"; let r = ToolRegistry::new(); }\nfn other() {}\n";
    let (f, _, n) = registry_failures("crates/x/src/a.rs", brace, &[]);
    assert_eq!(n, 1);
    assert!(
        f[0].contains("outer"),
        "the site is attributed to its fn: {f:?}"
    );
    // Lifetimes are not char literals.
    let lt = "fn f<'a>(x: &'a str) { let r = ToolRegistry::new(); }\n";
    assert_eq!(registry_failures("crates/x/src/a.rs", lt, &[]).2, 1);
}

#[test]
fn synthetic_gated_call_with_bare_bool_is_flagged() {
    let bad = "fn run() {\n    let _ = ironhermes_hooks::execute_gated_command(\"terminal\", cmd, &g, None, &a, sid, \"x\", cid, false, false, false, || async { Ok(String::new()) });\n}\n";
    let (sites, failures) = gated_call_report("crates/x/src/a.rs", bad);
    assert_eq!(sites, 1);
    assert_eq!(
        failures.len(),
        1,
        "a bare-bool policy must be flagged: {failures:?}"
    );

    let good = "fn run() {\n    let _ = ironhermes_hooks::execute_gated_command(\"terminal\", cmd, &g, None, &a, sid, \"x\", cid, ironhermes_hooks::GatePolicy::for_session(sid), false, false, || async { Ok(String::new()) });\n}\n";
    let (sites, failures) = gated_call_report("crates/x/src/a.rs", good);
    assert_eq!(sites, 1);
    assert!(
        failures.is_empty(),
        "inline for_session must pass: {failures:?}"
    );

    let bound = "fn run() {\n    let policy = ironhermes_hooks::GatePolicy::for_session(sid);\n    let _ = ironhermes_hooks::execute_gated_command(\"terminal\", cmd, &g, None, &a, sid, \"x\", cid, policy, false, false, || async { Ok(String::new()) });\n}\n";
    let (_, failures) = gated_call_report("crates/x/src/a.rs", bound);
    assert!(
        failures.is_empty(),
        "a let-bound for_session must pass: {failures:?}"
    );

    let bound_bad = "fn run() {\n    let policy = ironhermes_hooks::GatePolicy::fixed(lvl, None);\n    let _ = ironhermes_hooks::execute_gated_command(\"terminal\", cmd, &g, None, &a, sid, \"x\", cid, policy, false, false, || async { Ok(String::new()) });\n}\n";
    let (_, failures) = gated_call_report("crates/x/src/a.rs", bound_bad);
    assert_eq!(
        failures.len(),
        1,
        "a let-bound fixed policy must be flagged: {failures:?}"
    );

    let in_test = "#[cfg(test)]\nmod tests {\n    fn t() { execute_gated_command(\"terminal\", c, &g, None, &a, s, \"x\", c, false, false, false, || async { Ok(String::new()) }); }\n}\n";
    let (sites, failures) = gated_call_report("crates/x/src/a.rs", in_test);
    assert_eq!(
        (sites, failures.len()),
        (0, 0),
        "cfg(test) regions are not production"
    );

    let in_comment = "// execute_gated_command(\"terminal\", c, &g, None, &a, s, \"x\", c, false, false, false, run)\n";
    let (sites, _) = gated_call_report("crates/x/src/a.rs", in_comment);
    assert_eq!(sites, 0, "comments are not production");
}

#[test]
fn synthetic_typed_policy_parameter_needs_a_listed_exemption() {
    let src = "pub async fn handle_shell_exec(policy: ironhermes_hooks::GatePolicy) {\n    execute_gated_command(\"terminal\", c, &g, None, &a, s, \"gateway\", c, policy, false, false, run).await;\n}\n";
    let (_, listed) = gated_call_report("crates/ironhermes-gateway/src/handler.rs", src);
    assert!(
        listed.is_empty(),
        "the listed pass-through must pass: {listed:?}"
    );
    let (_, unlisted) = gated_call_report("crates/other/src/handler.rs", src);
    assert_eq!(
        unlisted.len(),
        1,
        "an unlisted typed pass-through must be flagged: {unlisted:?}"
    );
}

#[test]
fn synthetic_host_without_boot_call_is_flagged() {
    let spec = BootSpec {
        path: "crates/h/src/main.rs",
        boot_fn: "boot_security_for_host",
        body_needle: "boot(",
    };
    let wired = "fn boot_security_for_host() { let _s = boot(&opts); }\nfn main() { boot_security_for_host(); }\n";
    assert!(host_boot_failures(spec.path, wired, &spec).is_empty());

    let never_called = "fn boot_security_for_host() { let _s = boot(&opts); }\nfn main() { }\n";
    assert_eq!(host_boot_failures(spec.path, never_called, &spec).len(), 1);

    let hollow = "fn boot_security_for_host() { }\nfn main() { boot_security_for_host(); }\n";
    assert_eq!(host_boot_failures(spec.path, hollow, &spec).len(), 1);

    let commented = "fn boot_security_for_host() { let _s = boot(&opts); }\nfn main() { // boot_security_for_host();\n}\n";
    assert_eq!(host_boot_failures(spec.path, commented, &spec).len(), 1);

    let missing = "fn main() { }\n";
    assert_eq!(host_boot_failures(spec.path, missing, &spec).len(), 1);
}

#[test]
fn synthetic_yolo_readers_are_flagged() {
    assert_eq!(
        yolo_reader_failures("crates/x/src/a.rs", "let y = config.autonomous.yolo;\n").len(),
        1
    );
    assert_eq!(
        yolo_reader_failures(
            "crates/x/src/a.rs",
            "let y = config\n    .autonomous\n    .yolo;\n"
        )
        .len(),
        1,
        "a field chain split across lines is still a reader"
    );
    assert!(
        yolo_reader_failures(
            "crates/x/src/a.rs",
            "// the old autonomous.yolo flag\nlet a = 1;\n"
        )
        .is_empty()
    );
    assert!(
        yolo_reader_failures(
            "crates/x/src/a.rs",
            "#[cfg(test)]\nmod t { fn f() { let y = c.autonomous.yolo; } }\n"
        )
        .is_empty()
    );
    assert!(
        yolo_reader_failures(
            "crates/x/src/a.rs",
            "let y = config.autonomous.yolo_mode_x;\n"
        )
        .is_empty()
    );
}

#[test]
fn synthetic_unscreened_registry_is_flagged() {
    let bare = "fn build_thing() {\n    let r = ToolRegistry::new();\n}\n";
    let (f, _, n) = registry_failures("crates/x/src/a.rs", bare, &[]);
    assert_eq!(
        (n, f.len()),
        (1, 1),
        "an unscreened registry must be flagged: {f:?}"
    );
    assert!(
        f[0].contains("build_thing"),
        "the message names the enclosing fn: {f:?}"
    );

    let screened =
        "fn build_thing() {\n    let mut r = ToolRegistry::new();\n    r.set_tool_screen(s);\n}\n";
    let (f, _, n) = registry_failures("crates/x/src/a.rs", screened, &[]);
    assert_eq!((n, f.len()), (1, 0));

    let two_one_screen = "fn a() { let r = ToolRegistry::new(); r.set_tool_screen(s); }\nfn b() { let r = ToolRegistry::default(); }\n";
    let (f, _, _) = registry_failures("crates/x/src/a.rs", two_one_screen, &[]);
    assert_eq!(f.len(), 1, "two registries need two screens: {f:?}");

    let exempt = [("crates/x/src/a.rs", "build_thing", "inspection only")];
    let (f, used, _) = registry_failures("crates/x/src/a.rs", bare, &exempt);
    assert!(
        f.is_empty() && used == vec![0],
        "a listed fn is exempt and reported as used: {f:?} {used:?}"
    );

    let (f, used, _) = registry_failures("crates/x/src/b.rs", bare, &exempt);
    assert!(f.len() == 1 && used.is_empty(), "an exemption is per file");

    let in_test = "#[cfg(test)]\nmod tests { fn t() { let r = ToolRegistry::new(); } }\n";
    let (f, _, n) = registry_failures("crates/x/src/a.rs", in_test, &[]);
    assert_eq!((n, f.len()), (0, 0));

    let in_comment = "// let r = ToolRegistry::new();\n";
    let (_, _, n) = registry_failures("crates/x/src/a.rs", in_comment, &[]);
    assert_eq!(n, 0);
}

#[test]
fn synthetic_fake_backend_outside_tests_is_flagged() {
    let prod = "fn boot() { install_fake_for_tests(FakeBackend::new()); }\n";
    assert_eq!(fake_backend_failures("crates/x/src/a.rs", prod).len(), 1);
    let in_test =
        "#[cfg(test)]\nmod tests { fn t() { install_fake_for_tests(FakeBackend::new()); } }\n";
    assert!(fake_backend_failures("crates/x/src/a.rs", in_test).is_empty());
    assert!(fake_backend_failures("crates/ironhermes-security/src/testing.rs", prod).is_empty());
    assert!(
        fake_backend_failures(
            "crates/x/src/a.rs",
            "// install_fake_for_tests is test-only\n"
        )
        .is_empty()
    );
}

#[test]
fn synthetic_os_sandbox_spawn_is_flagged() {
    assert_eq!(
        os_sandbox_failures(
            "crates/x/src/a.rs",
            "let c = Command::new(\"sandbox-exec\");\n"
        )
        .len(),
        1
    );
    assert_eq!(
        os_sandbox_failures(
            "crates/x/src/a.rs",
            "let c = tokio::process::Command::new(\"bwrap\");\n"
        )
        .len(),
        1
    );
    assert_eq!(
        os_sandbox_failures(
            "crates/x/src/a.rs",
            "let c = Command::new(\n    \"bwrap\",\n);\n"
        )
        .len(),
        1,
        "a wrapped argument list is still a spawn"
    );
    assert!(os_sandbox_failures("crates/x/src/a.rs", "let c = Command::new(\"ls\");\n").is_empty());
    assert!(
        os_sandbox_failures(
            "crates/x/src/a.rs",
            "#[cfg(test)]\nmod t { fn f() { Command::new(\"bwrap\"); } }\n"
        )
        .is_empty()
    );
}
