//! Static-grep regression tests locking the six Phase 21 structural invariants
//! (RESEARCH.md §Invariants). These are intentionally brittle — if a future
//! refactor changes the structure, the test tells you exactly what invariant
//! was broken so you can either (a) fix the invariant or (b) update the test
//! with explicit justification.

use std::fs;
use std::path::PathBuf;

fn crate_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(path: &str) -> String {
    let full = crate_root().join(path);
    fs::read_to_string(&full).unwrap_or_else(|e| panic!("read {:?}: {}", full, e))
}

/// Extract the body of a top-level `async fn NAME` block from main.rs.
/// Matches from `async fn NAME` through the first balanced `}` at indent 0.
fn extract_fn_body(src: &str, name: &str) -> String {
    let needle = format!("async fn {}", name);
    let start = src.find(&needle).unwrap_or_else(|| {
        panic!("function `async fn {}` not found in main.rs", name)
    });
    let bytes = src.as_bytes();
    let mut i = start;
    while i < bytes.len() && bytes[i] != b'{' {
        i += 1;
    }
    if i >= bytes.len() {
        panic!("opening brace for {} not found", name);
    }
    let body_start = i;
    let mut depth = 0i32;
    while i < bytes.len() {
        match bytes[i] {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return src[body_start..=i].to_string();
                }
            }
            _ => {}
        }
        i += 1;
    }
    panic!("closing brace for {} not found", name);
}

#[test]
fn inv_1_run_chat_has_tokio_select_with_ctrl_c() {
    let src = read("src/main.rs");
    let run_chat = extract_fn_body(&src, "run_chat");
    assert!(
        run_chat.contains("tokio::signal::ctrl_c"),
        "INV-1: run_chat must wrap in-flight agent future with tokio::signal::ctrl_c — not found"
    );
    assert!(
        run_chat.contains("tokio::select!"),
        "INV-1: run_chat must use tokio::select! for ctrl-c handling — not found"
    );
}

#[test]
fn inv_2_fresh_child_token_per_turn() {
    let src = read("src/main.rs");
    let run_chat = extract_fn_body(&src, "run_chat");
    assert!(
        run_chat.contains("child_token()"),
        "INV-2: run_chat must issue fresh child CancellationToken per turn (RESEARCH §Pitfall 2) — child_token() not found"
    );
    assert!(
        run_chat.contains("chat_cancel_parent"),
        "INV-2: expected chat_cancel_parent parent-token name — not found"
    );
}

#[test]
fn inv_3_run_single_does_not_install_ctrl_c() {
    let src = read("src/main.rs");
    let run_single = extract_fn_body(&src, "run_single");
    assert!(
        !run_single.contains("tokio::signal::ctrl_c"),
        "INV-3: run_single must NOT install ctrl-c handler (D-10)"
    );
    assert!(
        !run_single.contains("DoubleCtrlCState"),
        "INV-3: run_single must NOT use the double-ctrl-c state machine (D-10)"
    );
}

#[test]
fn inv_4_render_pairs_save_and_restore_position() {
    let render = read("src/tui/render.rs");
    let saves = render.matches("SavePosition").count();
    let restores = render.matches("RestorePosition").count();
    assert!(
        saves >= 1 && restores >= 1,
        "INV-4: tui/render.rs must use both SavePosition and RestorePosition — saves={}, restores={}",
        saves, restores
    );
    assert!(
        restores >= saves,
        "INV-4: every SavePosition should have a matching RestorePosition (saves={} restores={})",
        saves, restores
    );
}

#[test]
fn inv_5_no_stdout_prints_in_tui_module() {
    let tui_dir = crate_root().join("src/tui");
    let entries = fs::read_dir(&tui_dir)
        .unwrap_or_else(|e| panic!("read_dir {:?}: {}", tui_dir, e));
    for entry in entries {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("rs") {
            continue;
        }
        let src = fs::read_to_string(&path).unwrap();
        // Split the file at `#[cfg(test)]` — only check everything BEFORE the
        // first test module annotation. This is a conservative heuristic:
        // production code is written above tests by convention.
        let prod_slice = match src.find("#[cfg(test)]") {
            Some(idx) => &src[..idx],
            None => &src[..],
        };
        for (lineno, line) in prod_slice.lines().enumerate() {
            let trimmed = line.trim_start();
            if trimmed.starts_with("println!") || trimmed.starts_with("print!(") {
                panic!(
                    "INV-5: println!/print! found in {:?} line {} (production code): {}",
                    path,
                    lineno + 1,
                    line
                );
            }
        }
    }
}

#[test]
fn inv_6_no_forbidden_new_deps_in_cargo_toml() {
    let cargo = read("Cargo.toml");
    for forbidden in &["ratatui", "reedline", "ctrlc = ", "signal-hook"] {
        assert!(
            !cargo.contains(forbidden),
            "INV-6: forbidden dep `{}` found in Cargo.toml (D-18: no new deps this phase)",
            forbidden
        );
    }
}
