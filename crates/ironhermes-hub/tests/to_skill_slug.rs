//! Golden-vector integration test for `to_skill_slug` (ports
//! `blob.ts:55-62` byte-for-byte).
//!
//! A single drift in the slug algorithm produces silent 404s against
//! `skills.sh/api/download/.../<slug>`; this test catches divergence
//! between our Rust port and the reference TypeScript algorithm.

mod fixtures;

use fixtures::load_slug_vectors;
use ironhermes_hub::to_skill_slug;

#[test]
fn to_skill_slug_golden_vectors() {
    let vectors = load_slug_vectors();
    assert!(
        vectors.len() >= 20,
        "need at least 20 golden vectors, found {}",
        vectors.len()
    );

    let mut failures = Vec::new();
    for (input, expected) in &vectors {
        let actual = to_skill_slug(input);
        if actual != *expected {
            failures.push(format!(
                "input={input:?} expected={expected:?} actual={actual:?}"
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "slug golden-vector failures:\n{}",
        failures.join("\n")
    );
}
