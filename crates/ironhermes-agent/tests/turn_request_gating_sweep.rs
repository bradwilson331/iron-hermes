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

    let file_names: Vec<String> = sites
        .iter()
        .map(|(f, _)| f.display().to_string())
        .collect();
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
