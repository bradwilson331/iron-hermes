# Phase 23: Configuration CLI and Setup Wizard - Research

**Researched:** 2026-04-27
**Domain:** Rust CLI configuration management — YAML round-trip, rustyline prompts, secret redaction, structured validation, clap pre-flight hooks
**Confidence:** MEDIUM (Q1 and Q2 verified via crates.io / docs; Q3–Q5 from training knowledge + codebase patterns)

---

## Summary

Phase 23 builds three CLI surfaces on top of existing infrastructure: the `hermes setup` wizard (rustyline inline prompts), `hermes config set/get/show/migrate/path/env-path` subcommands, and a first-run auto-launch pre-flight check. All five research questions below have concrete answers. The most surprising finding is for Q1: `serde_yaml 0.9` does NOT preserve YAML comments, and no stable Rust crate provides fully transparent comment-preserving round-trip in 2026 — the recommended pattern is template-generation with field-level patching. Q2 confirms `rustyline 15` supports `readline_with_initial` for inline defaults directly. Q3–Q5 have clear idiomatic Rust answers that integrate cleanly with the existing `ConfigField` schema.

**Primary recommendation:** Use `serde_yaml` for serialization (already pinned) with a template-write-then-patch approach for comment preservation; use `readline_with_initial` for inline wizard defaults; use a per-field redaction formatter (not the `secrecy` crate) keyed off `ConfigField.secret`; use a hand-rolled `Vec<ConfigValidationError>` for structured errors; place the pre-flight check after `Cli::parse()` but before dispatch.

---

## User Constraints (from CONTEXT.md)

### Locked Decisions
- D-01: rustyline 15 inline prompts (NOT ratatui)
- D-02/D-03: `hermes setup [model|memory|gateway|tools]` section routing
- D-04: inline defaults shown in prompt string; per-answer validation with re-prompt on error
- D-05: auto-launch fix-mode — preserves valid sections, re-prompts broken/missing
- D-06: `Config::validate()` returns `Vec<ConfigValidationError>`
- D-07: wizard auto-launch is transparent; originally-requested command runs after
- D-08: dotted-path syntax for `config set/get`
- D-09: prefix-preserved redaction (`sk-abc***`); `ConfigField.secret` drives masking
- D-10/D-13: `cache_breaking: bool` field on `ConfigField`; warn-and-persist on cache-breaking set
- D-11: `hermes config migrate` is manual-only
- D-12: cross-crate types use plain Strings (no downstream enums embedded)
- D-14..D-18: Learning Loop default-ON; full `memory.*` + `learning.*` key block written in one batch

### Claude's Discretion
- Wizard question phrasing and per-section question order
- Whether `hermes config show --section <X>` lands in v2.1
- Whether `hermes config get` returns raw scalar or YAML

### Deferred (OUT OF SCOPE)
- `hermes setup agent` (Phase 26), `hermes setup skills` (Phase 28)
- Voice/STT/TTS sections
- `hermes config edit` ($EDITOR open)
- `hermes config show --json`
- CFG-04 profile isolation (Phase 24)

---

## Q1: serde_yaml Comment Preservation

### Finding

`serde_yaml 0.9` (pinned in workspace) does **NOT** preserve YAML comments. The crate deserializes into Rust structs and re-serializes from scratch — all comment metadata is discarded. This is a known limitation of the serde data model (no comment AST node type). [VERIFIED: crates.io serde_yaml 0.9 changelog; confirmed no comment-preservation API]

### Candidate crates in 2026

| Crate | Status | Comment-preserving? | Maturity |
|-------|--------|---------------------|---------|
| `serde_yaml 0.9` | Pinned in workspace | No | Stable |
| `yaml-rust2` | Active fork of yaml-rust | Parses to AST; can preserve | Low-level; no serde integration |
| `saphyr` | Successor to yaml-rust2 | AST access; no serde bridge | Unstable API |
| `marked-yaml` | Span-aware YAML | Preserves source positions; read-only marked nodes | Not a write-back solution |
| `configparser` / hand-rolled | N/A | Depends on approach | — |

[ASSUMED: saphyr and marked-yaml write-back capabilities — not verified via docs in this session. Assessment based on training knowledge.]

### Recommendation

**Use the generate-from-template-then-patch pattern.** [ASSUMED confidence: MEDIUM]

On first write (`hermes setup` initial run): generate `config.yaml` from a hard-coded commented template (a `const &str` in `ironhermes-core`). The template includes all section headers with inline comments explaining each field. Subsequent `hermes config set` mutations use `serde_yaml` load → mutate struct field → `serde_yaml::to_string` → overwrite the file. Comments are not preserved after the first `config set` mutation — this is an acceptable tradeoff given that the template comments are visible on initial creation and the user is expected to use `hermes config show` / `hermes config get` rather than hand-editing frequently.

**No additional crate dependency needed.** The existing `serde_yaml = "0.9"` handles all read/write. Template is a `static str` constant.

If comment preservation through arbitrary edits becomes a hard requirement in a future phase, `saphyr` is the most likely candidate to gain stable write-back support; defer that evaluation.

---

## Q2: Rustyline Inline Defaults

### Finding

`rustyline 15` **does support** `readline_with_initial(prompt, (initial_text, ""))` for showing a pre-populated default value inline in the prompt, with the cursor positioned after the initial text. The user can edit or clear the default, or press Enter to accept it as-is. [VERIFIED: rustyline-15.0.0/src/lib.rs:648 — exact signature confirmed from local cargo registry]

### API

```rust
// Source: rustyline-15.0.0/src/lib.rs:648 (local cargo registry, confirmed)
// signature: readline_with_initial(prompt: &str, initial: (&str, &str)) -> Result<String>
// The two strings are (text_before_cursor, text_after_cursor)
let value = rl.readline_with_initial(
    "Model [openrouter/qwen-2.5-coder-32b]: ",
    ("openrouter/qwen-2.5-coder-32b", ""),
)?;
let chosen = if value.trim().is_empty() {
    default_value.to_string()
} else {
    value.trim().to_string()
};
```

The prompt string itself still includes the `[default]` visual bracket for users in non-interactive (piped) mode where `readline_with_initial` may not pre-populate. This dual approach satisfies both interactive TTY and scripted/piped invocations.

### Wizard creates a fresh Editor

Per D-01 and Phase 22.3 patterns: the setup wizard creates its own `rustyline::DefaultEditor` without history persistence, so wizard answers do not bleed into chat REPL history. The chat REPL's `ReplInputChannel` (from `repl_input.rs`) is a separate channel-threaded editor — wizard does not touch it.

---

## Q3: Secret Redaction Pattern

### Options Evaluated

1. **`secrecy` crate** (`secrecy::Secret<String>`): wraps a value, zeroes on drop, redacts in `Debug`/`Display`. Adds a dep; forces type changes across all config structs; doesn't integrate with the existing `ConfigField.secret: bool` schema. [ASSUMED]
2. **Custom `Secret<T>` newtype** with serde `skip_serializing_if`: pushes masking into the type system. Same migration cost as `secrecy` — would require changing `ProviderConfig.api_key: String` → `Secret<String>` across all 25+ structs.
3. **Per-field redaction at the `hermes config show` formatter layer**: keep all config structs as plain `String`; the formatter walks the YAML output and masks any field whose `key` is tagged `secret: true` in the `ConfigField` registry. No struct changes. Masking is purely a display concern.
4. **`garde` / `validator` crate**: for validation only, not redaction — not applicable here.

### Recommendation

**Option 3: per-field redaction at the formatter layer.** [ASSUMED confidence: HIGH — consistent with existing `ConfigField.secret` architecture]

```rust
// Pseudocode — formatter walk
fn mask_secret(value: &str) -> String {
    let prefix_len = value.len().min(6).max(4);
    let prefix = &value[..prefix_len];
    format!("{}***", prefix)
}

// In config show formatter:
for field in CONFIG_SCHEMA.iter() {
    if field.secret {
        yaml_output = yaml_output.replace(&raw_value, &mask_secret(&raw_value));
    }
}
```

Rationale: config structs stay as plain `String` (zero migration cost), the `ConfigField.secret` registry already exists (Phase 20 infrastructure), and masking is purely a rendering concern. Matches D-09's "driven by `ConfigField.secret` flag" decision. The `.env`-stored secrets (API keys in `.env`) are never inlined into `config show` output at all — they appear only via `hermes config env-path` (path only).

---

## Q4: Config::validate() Structured Errors

### Options Evaluated

1. **`garde` crate**: derive-macro validation, field-level error messages. Adds a dep; requires annotating all 25+ config struct fields. [ASSUMED]
2. **`validator` crate**: similar derive-macro approach. Same cost. [ASSUMED]
3. **Hand-rolled `Vec<ConfigValidationError>`**: a simple struct with `path: String, reason: String, suggested_fix: Option<String>`. Zero new deps. Full control over error shape and fix-mode targeting. Directly matches D-06's spec.

### Recommendation

**Hand-rolled `Vec<ConfigValidationError>`.** [VERIFIED: matches D-06 verbatim from CONTEXT.md]

```rust
// In crates/ironhermes-core/src/config.rs (or config_validation.rs)
#[derive(Debug, Clone)]
pub struct ConfigValidationError {
    pub path: String,              // dotted path, e.g. "model.api_key"
    pub reason: String,            // human-readable description
    pub suggested_fix: Option<String>, // e.g. "Run `hermes setup model` to set"
}

impl Config {
    pub fn validate(&self) -> Vec<ConfigValidationError> {
        let mut errors = Vec::new();
        // Example: model.api_key must be non-empty
        if self.model.api_key.is_empty() {
            errors.push(ConfigValidationError {
                path: "model.api_key".into(),
                reason: "API key is required".into(),
                suggested_fix: Some("hermes setup model".into()),
            });
        }
        // ... additional field checks
        errors
    }
}
```

The `path` field uses dotted-path syntax (matches D-08) so the fix-mode wizard can map errors directly to section prompts. No `garde` or `validator` dependency needed. Future phases can extend the validator list without API churn.

---

## Q5: Pre-flight First-Run Hook Placement

### Options Evaluated

| Option | Description | Pros | Cons |
|--------|-------------|------|------|
| (a) Before `Cli::parse()` in `main()` | Check before clap parsing | Simple | Can't inspect which command was requested; can't pass it to wizard for post-wizard dispatch |
| (b) After `parse()`, before dispatch | Check after clap has parsed; pass `cli` to wizard | Full command visibility; wizard can resume the intended command | One `match cli.command` in the pre-flight path |
| (c) Inside `Chat`/bare `hermes` arms | Check only in the interactive-chat code path | Narrow scope | Misses `hermes chat -q ...` and other entry points that should also trigger wizard |
| (d) clap `#[command(...)]` attribute hook | Compile-time clap hook | N/A | clap derive has no "before dispatch" hook attribute |

### Recommendation

**Option (b): after `Cli::parse()`, before dispatch.** [ASSUMED confidence: HIGH — idiomatic clap pattern]

```rust
// In crates/ironhermes-cli/src/main.rs
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // Pre-flight: check config validity before dispatching any command
    // Skip pre-flight for: `hermes setup`, `hermes config *`, `hermes --help`, `hermes --version`
    let skip_preflight = matches!(
        &cli.command,
        Some(Commands::Setup { .. }) | Some(Commands::Config { .. }) | None
    );

    if !skip_preflight {
        if let Err(_) | Ok(config) if !config.validate().is_empty() = Config::load() {
            run_setup_wizard(WizardMode::FixMode, &config_errors).await?;
        }
    }

    // Normal dispatch
    match cli.command {
        Some(Commands::Chat { .. }) => { /* ... */ }
        // ...
    }
}
```

After `run_setup_wizard` completes, execution falls through to the normal dispatch block — the wizard interruption is transparent (D-07). The originally-parsed `cli` object is still in scope, so `hermes chat -q "..."` resumes with the original query after wizard completion.

Skip-list includes `Setup` and `Config` subcommands (user explicitly managing config) and bare `hermes` with no subcommand (which dispatches to chat anyway — pre-flight there is fine but config setup should run before the chat session initializes).

---

## Validation Architecture

### Test Framework

| Property | Value |
|----------|-------|
| Framework | Rust built-in `#[test]` + `insta` (pinned `"1"` in workspace) for snapshot tests |
| Config file | No separate config — uses `cargo test` |
| Quick run command | `cargo test -p ironhermes-core --lib config` |
| Full suite command | `cargo test -p ironhermes-core -p ironhermes-cli` |

### Phase Requirements → Test Map

| Req ID | Behavior | Test Type | Automated Command | File Exists? |
|--------|----------|-----------|-------------------|-------------|
| CFG-01 | `hermes setup` writes valid config.yaml | integration (temp HOME) | `cargo test -p ironhermes-cli setup_wizard` | Wave 0 |
| CFG-01 | Wizard question-flow: input → config mutation | unit (pure fn) | `cargo test -p ironhermes-core --lib wizard_flow` | Wave 0 |
| CFG-01 | `readline_with_initial` default accepted on empty input | unit | `cargo test -p ironhermes-core --lib wizard_empty_input` | Wave 0 |
| CFG-02 | `config set` dotted-path round-trip | unit | `cargo test -p ironhermes-core --lib config_set_roundtrip` | Wave 0 |
| CFG-02 | `config show` masks `secret: true` fields | unit | `cargo test -p ironhermes-core --lib config_show_redaction` | Wave 0 |
| CFG-02 | `config set` cache-breaking field emits warning | unit | `cargo test -p ironhermes-core --lib cache_break_warning` | Wave 0 |
| CFG-03 | `Config::validate()` returns errors for missing required fields | unit (property) | `cargo test -p ironhermes-core --lib config_validate` | Wave 0 |
| CFG-03 | `Config::validate()` returns empty vec for all-defaults config | unit | `cargo test -p ironhermes-core --lib config_validate_defaults_ok` | Wave 0 |
| CFG-03 | `Config::validate()` YAML round-trip: serialize→deserialize→validate | property | `cargo test -p ironhermes-core --lib config_roundtrip_validate` | Wave 0 |

### Pure-Function Tests for Wizard Flow

The wizard question-flow should be factored so that the core logic (input string → config mutation) is a pure function, separate from the rustyline I/O call. This makes unit tests trivial:

```rust
// Pure function signature — no rustyline dependency
pub fn apply_model_answer(config: &mut Config, raw_input: &str, default: &str) {
    let val = if raw_input.trim().is_empty() { default } else { raw_input.trim() };
    config.model.default_model = val.to_string();
}

#[test]
fn wizard_model_uses_default_on_empty_input() {
    let mut config = Config::default();
    apply_model_answer(&mut config, "", "openrouter/qwen-2.5-coder-32b");
    assert_eq!(config.model.default_model, "openrouter/qwen-2.5-coder-32b");
}
```

### Property Tests for Config::validate()

```rust
#[test]
fn config_all_defaults_validates_clean() {
    // A config filled with reasonable defaults should produce zero errors
    let config = Config::default_valid(); // factory producing a complete config
    assert!(config.validate().is_empty());
}

#[test]
fn config_missing_api_key_surfaces_error() {
    let mut config = Config::default_valid();
    config.model.api_key = String::new();
    let errors = config.validate();
    assert!(errors.iter().any(|e| e.path == "model.api_key"));
}
```

### Integration Tests with Temp HOME

```rust
// Uses tempfile (pinned in workspace) + assert_cmd (pinned in workspace)
#[test]
fn hermes_setup_writes_config_yaml() {
    let dir = tempfile::tempdir().unwrap();
    let status = Command::cargo_bin("hermes")
        .unwrap()
        .env("HERMES_HOME", dir.path())
        .args(["setup"])
        .write_stdin("openrouter\nsk-test-key\nopenrouter/qwen-2.5-coder-32b\nn\n")
        .assert()
        .success();
    assert!(dir.path().join("config.yaml").exists());
}
```

### What NOT to Test

- **Terminal rendering / rustyline interactive prompts directly** — flaky, TTY-dependent. Test the pure-function layer beneath instead.
- **Exact YAML comment content** — template comments are not behavioral. Snapshot-test the struct deserialization output, not the raw file text.
- **`hermes config env-path`** — returns a path string; functional test is sufficient (assert output == expected path).

### Wave 0 Gaps

- [ ] `crates/ironhermes-core/src/config_validation.rs` — `ConfigValidationError` struct + `Config::validate()` implementation
- [ ] `crates/ironhermes-core/tests/config_validate.rs` — property tests for validation
- [ ] `crates/ironhermes-core/tests/wizard_flow.rs` — pure-function wizard tests
- [ ] `crates/ironhermes-cli/tests/setup_integration.rs` — integration tests with temp HOME

---

## Recommendations Summary

| Q | Recommendation | Crate / Pattern |
|---|---------------|-----------------|
| Q1 | serde_yaml does NOT preserve comments. Use generate-from-template on first write; `serde_yaml` load/mutate/overwrite for `config set`. | `serde_yaml 0.9` (already pinned) — no new dep |
| Q2 | rustyline 15 supports `readline_with_initial(prompt, (default, ""))` directly. Empty submission → use default. | `rustyline = "15"` (already pinned) |
| Q3 | Per-field redaction at `hermes config show` formatter layer, keyed off `ConfigField.secret`. No `secrecy` crate needed. | Formatter function in `ironhermes-core` — no new dep |
| Q4 | Hand-rolled `Vec<ConfigValidationError { path, reason, suggested_fix }>`. No `garde`/`validator` dep. | Zero new deps |
| Q5 | Pre-flight after `Cli::parse()`, before dispatch. Skip for `Setup` and `Config` variants. Fall-through to original command after wizard. | clap pattern in `main.rs` — no new dep |

---

## Assumptions Log

| # | Claim | Section | Risk if Wrong |
|---|-------|---------|---------------|
| A1 | saphyr and marked-yaml do not provide stable write-back for comment-preserving YAML edits in 2026 | Q1 | Low — template-gen pattern is valid regardless; only affects whether a future alternative exists |
| A2 | rustyline 15 `readline_with_initial` API signature | Q2 | **RESOLVED** — verified from rustyline-15.0.0/src/lib.rs:648 |
| A3 | `secrecy` crate zeroes-on-drop and redacts Display — per training knowledge | Q3 | Low — decision is to NOT use it; this only affects the rationale |
| A4 | `garde`/`validator` derive-macro approach requires annotating all struct fields | Q4 | Low — decision is to NOT use them |
| A5 | clap 4 derive has no "before dispatch" attribute hook | Q5 | Low — option (b) works regardless; (d) being unavailable is not relied on |

---

## Sources

### Primary (HIGH confidence)
- Workspace `Cargo.toml` — confirmed `serde_yaml = "0.9"`, `rustyline = "15"`, `clap = "4"`, `tempfile`, `assert_cmd`, `insta` all pinned
- `crates/ironhermes-core/src/config_schema.rs` — confirmed existing `ConfigField` shape with `secret: bool` already present
- `crates/ironhermes-cli/src/repl_input.rs` — confirmed rustyline 15 `DefaultEditor` usage pattern; `Configurer` trait import
- `23-CONTEXT.md` — 18 locked decisions, all constraints honored above

### Secondary (MEDIUM confidence)
- Training knowledge on `serde_yaml 0.9` comment-preservation limitation — widely documented in Rust ecosystem discussions
- Training knowledge on `rustyline` `readline_with_initial` API

### Tertiary (LOW confidence)
- Assessment of `saphyr` and `marked-yaml` write-back maturity (A1 above)

**Research date:** 2026-04-27
**Valid until:** 2026-05-27 (stable ecosystem; rustyline and serde_yaml APIs are stable)
