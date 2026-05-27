
## 2026-05-27 (Plan 04 — clippy out-of-scope)

`cargo clippy -p ironhermes-gateway -- -D warnings` transitively compiles
`ironhermes-core`, which emits 15 `collapsible_if` errors (rust-clippy 1.94.0)
across `crates/ironhermes-core/src/skills.rs` and unrelated files. None are in
files touched by Plan 04. SCOPE BOUNDARY: not a Plan 04 regression — pre-existing
on `develop` base `0069d1702cc642b6e05d0d36f7e8a5bb3a46bac7`. Gateway-only verify
spec for Plan 04 (`cargo test -p ironhermes-gateway`) is green.

Recommended follow-up phase: `cargo clippy --fix -p ironhermes-core --lib --tests`
or hand-collapse the 15 `if let ... { if ... { ... } }` blocks per clippy 1.94's
new `collapsible_if` enforcement.
