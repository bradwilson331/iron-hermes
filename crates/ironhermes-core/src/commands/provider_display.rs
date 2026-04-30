//! Shared provider display helpers — Phase 26, D-14.
//!
//! Lives in `ironhermes-core` so BOTH the CLI subcommand (`provider_cmd.rs`)
//! and the slash command handler (Phase 21.1 CommandRouter) can call it
//! without creating a circular dependency (ironhermes-cli → ironhermes-core,
//! NEVER the reverse).
//!
//! Pure functions: no I/O, no environment access, NO api_key VALUES — only
//! env var NAMES. T-26-01 mitigation by struct shape (ProviderRow has no
//! field that can carry a key value).

use serde::Serialize;

/// A single row in the `hermes provider list` aligned-columns table.
///
/// **T-26-01 by construction:** this struct has NO field for an API key VALUE.
/// `api_key_status` carries only the env var NAME (e.g. `"✓ $OPENAI_API_KEY"`
/// or `"✗ missing $OPENAI_API_KEY"`). The caller (`provider_cmd.rs`) constructs
/// this string from `endpoint.api_key.is_some()` and `config.providers[name].api_key_env`.
/// The key value itself never touches this struct.
#[derive(Debug, Clone, Serialize)]
pub struct ProviderRow {
    pub name: String,
    pub base_url: String,
    /// Either `"✓ $OPENAI_API_KEY"` or `"✗ missing $OPENAI_API_KEY"` — env var
    /// NAME only. NEVER the key VALUE. Constructed by the caller (provider_cmd.rs)
    /// from ResolvedEndpoint introspection.
    pub api_key_status: String,
    pub default_model: String,
    pub role: String,      // "main" / "aux" / "—"
    pub fallbacks: String, // comma-separated names or "—"
    pub disabled: bool,
}

/// Render the aligned-columns table for `hermes provider list` / `/provider list`.
///
/// Column widths per CONTEXT.md §Specifics / PATTERNS.md:
/// NAME=18, BASE_URL=36, API_KEY=22, MODEL=20, ROLE=10, FALLBACKS=remainder.
///
/// Pure function — no I/O.
pub fn render_provider_list(rows: Vec<ProviderRow>) -> String {
    let header = format!(
        "{:<18} {:<36} {:<22} {:<20} {:<10} {}\n",
        "NAME", "BASE_URL", "API_KEY", "MODEL", "ROLE", "FALLBACKS"
    );
    let mut out = header;
    for row in &rows {
        let name_disp = if row.disabled {
            format!("{} (disabled)", row.name)
        } else {
            row.name.clone()
        };
        out.push_str(&format!(
            "{:<18} {:<36} {:<22} {:<20} {:<10} {}\n",
            name_disp,
            row.base_url,
            row.api_key_status,
            row.default_model,
            row.role,
            row.fallbacks,
        ));
    }
    out
}

/// Render the detail view for a single provider.
///
/// Pure function — no I/O.
pub fn render_provider_show(row: &ProviderRow) -> String {
    let status = if row.disabled { "disabled" } else { "enabled" };
    format!(
        "Provider:  {}\n  Base URL:  {}\n  API key:   {}\n  Model:     {}\n  Role:      {}\n  Fallbacks: {}\n  Status:    {}\n",
        row.name,
        row.base_url,
        row.api_key_status,
        row.default_model,
        row.role,
        row.fallbacks,
        status,
    )
}

/// Optional JSON output — D-14 `--json` flag mirrors hermes toolset.
pub fn render_provider_list_json(rows: &[ProviderRow]) -> Result<String, serde_json::Error> {
    serde_json::to_string_pretty(rows)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_row() -> ProviderRow {
        ProviderRow {
            name: "openai".to_string(),
            base_url: "https://api.openai.com/v1".to_string(),
            api_key_status: "\u{2713} $OPENAI_API_KEY".to_string(),
            default_model: "gpt-4o".to_string(),
            role: "main".to_string(),
            fallbacks: "\u{2014}".to_string(),
            disabled: false,
        }
    }

    #[test]
    fn render_provider_list_aligned_columns() {
        let out = render_provider_list(vec![sample_row()]);
        // Header starts with NAME padded to 18 chars
        assert!(
            out.starts_with("NAME              "),
            "header alignment: {:?}",
            out
        );
        // Row name padded to 18
        assert!(
            out.contains("openai            "),
            "row alignment: {:?}",
            out
        );
        // Key status visible
        assert!(
            out.contains("OPENAI_API_KEY"),
            "key status visible: {:?}",
            out
        );
        // Header columns present
        assert!(out.contains("BASE_URL"), "BASE_URL column: {:?}", out);
        assert!(out.contains("API_KEY"), "API_KEY column: {:?}", out);
        assert!(out.contains("MODEL"), "MODEL column: {:?}", out);
        assert!(out.contains("ROLE"), "ROLE column: {:?}", out);
        assert!(out.contains("FALLBACKS"), "FALLBACKS column: {:?}", out);
    }

    #[test]
    fn render_provider_list_no_key_value() {
        // T-26-01: the rendered output must never contain sk-* substrings.
        // The struct field api_key_status may only carry env var NAMES.
        let out = render_provider_list(vec![sample_row()]);
        assert!(
            !out.contains("sk-"),
            "rendered output must not contain sk-* prefix; got: {}",
            out
        );
    }

    #[test]
    fn render_provider_show_single_provider() {
        let out = render_provider_show(&sample_row());
        assert!(out.contains("Provider:"), "missing Provider label");
        assert!(out.contains("openai"), "missing provider name");
        assert!(out.contains("Base URL:"), "missing Base URL label");
        assert!(out.contains("API key:"), "missing API key label");
        assert!(out.contains("Fallbacks:"), "missing Fallbacks label");
        // T-26-01: no key value leakage
        assert!(!out.contains("sk-"), "sk- prefix must not appear in show output");
    }

    #[test]
    fn render_provider_list_disabled_shows_label() {
        let mut row = sample_row();
        row.disabled = true;
        let out = render_provider_list(vec![row]);
        assert!(
            out.contains("(disabled)"),
            "disabled provider should show (disabled) label: {}",
            out
        );
    }
}
