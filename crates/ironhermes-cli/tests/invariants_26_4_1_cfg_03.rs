//! Phase 26.4.1 CFG-03 source-text regression invariants.
//!
//! Locks the `run_preflight` gate widening to include `Commands::Gateway`
//! while preserving the two SIBLING gates (`is_interactive_repl`,
//! `is_chat_or_bare`) that must NOT widen — gateway keeps the
//! `ironhermes=info` log filter.
//!
//! Sibling-pattern reference: `crates/ironhermes-cli/tests/invariants_22_4.rs`.

const MAIN_RS: &str = include_str!("../src/main.rs");

/// CFG-03 INV-01: run_preflight gate widened to include Gateway.
#[test]
fn gate_widened_to_gateway_for_run_preflight() {
    let pattern =
        "Some(Commands::Chat { .. }) | Some(Commands::Gateway { .. }) | None";
    let matches: Vec<_> = MAIN_RS.match_indices(pattern).collect();
    assert_eq!(
        matches.len(),
        1,
        "Phase 26.4.1 CFG-03: expected EXACTLY ONE occurrence of the widened gate \
         '{}' in main.rs (the run_preflight binding). Found {} matches at offsets {:?}.",
        pattern,
        matches.len(),
        matches.iter().map(|(i, _)| i).collect::<Vec<_>>()
    );

    // The match must be associated with `run_preflight`. We look for
    // `let run_preflight` within 200 chars BEFORE the matched index.
    let (idx, _) = matches[0];
    let lookback_start = idx.saturating_sub(200);
    let lookback = &MAIN_RS[lookback_start..idx];
    assert!(
        lookback.contains("let run_preflight"),
        "Phase 26.4.1 CFG-03: widened gate must be the binding for `run_preflight`. \
         Lookback window did not contain 'let run_preflight':\n{}",
        lookback
    );
}

/// CFG-03 INV-02: sibling gates (is_interactive_repl, is_chat_or_bare) MUST NOT widen.
#[test]
fn sibling_gates_not_widened_to_gateway() {
    // Two un-widened occurrences (Phase 23 invariant): both sibling gates.
    let unwidened = "matches!(cli.command, Some(Commands::Chat { .. }) | None)";
    let unwidened_alt = "matches!(&cli.command, Some(Commands::Chat { .. }) | None)";
    let count = MAIN_RS.matches(unwidened).count() + MAIN_RS.matches(unwidened_alt).count();
    assert_eq!(
        count,
        2,
        "Phase 26.4.1 CFG-03: expected EXACTLY TWO un-widened sibling gates \
         (is_interactive_repl + is_chat_or_bare). Found {} matches across both \
         un-widened forms. Widening either of those to Gateway would suppress \
         the `ironhermes=info` log filter on gateway startup — out of CFG-03 scope.",
        count
    );

    // Make sure each sibling binding is still present.
    assert!(
        MAIN_RS.contains("let is_interactive_repl"),
        "is_interactive_repl binding must still exist in main.rs"
    );
    assert!(
        MAIN_RS.contains("let is_chat_or_bare"),
        "is_chat_or_bare binding must still exist in main.rs"
    );
}

/// CFG-03 INV-03: amendment doc comment present near the gate.
#[test]
fn phase_amendment_doc_comment_present() {
    let gate_idx = MAIN_RS
        .find("Some(Commands::Chat { .. }) | Some(Commands::Gateway { .. }) | None")
        .expect("widened gate must exist");
    // Look at the 1500 chars preceding the gate (the doc-comment block).
    let start = gate_idx.saturating_sub(1500);
    let preamble = &MAIN_RS[start..gate_idx];
    assert!(
        preamble.contains("Phase 26.4.1") || preamble.contains("CFG-03"),
        "Phase 26.4.1 CFG-03: expected an amendment doc comment referencing \
         'Phase 26.4.1' or 'CFG-03' within the 1500 chars preceding the widened \
         gate. Preamble was:\n{}",
        preamble
    );
}
