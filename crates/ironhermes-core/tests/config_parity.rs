//! Field-list drift test scaffold for cli-config.yaml.example vs typed Config sections.
//! REQ-37.1-02: Every top-level Config section must appear as a top-level key in
//! cli-config.yaml.example. These tests are INTENTIONALLY RED (Wave 0 scaffolding)
//! because cli-config.yaml.example is missing mcp_servers, autonomous, browser,
//! extract, prompt_caching, kanban, dashboard, tts, audio_cache.
//! They will be made GREEN by Plan 03 (cli-config.yaml.example parity).
//!
//! G3 GUARD (REQ-37.1-03): `example_contains_no_private_range_ips` and
//! `detector_fires_on_synthetic_private_ip` enforce that the shipped example carries
//! no RFC-1918 private-range IP address. The example is the seed source that
//! install.sh copies verbatim into every fresh ~/.ironhermes/config.yaml — a private
//! host in it leaks the developer's LAN topology to every external installer, and
//! D-11 never-clobber then preserves it across all future updates.

use ironhermes_core::config::Config;

/// Load cli-config.yaml.example as a raw serde_yaml::Value.
/// Path is relative to the crate manifest dir, two levels up to the workspace root.
fn load_example_yaml() -> serde_yaml::Value {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../cli-config.yaml.example");
    let contents = std::fs::read_to_string(path).expect("cli-config.yaml.example must exist");
    serde_yaml::from_str(&contents).expect("cli-config.yaml.example must be valid YAML")
}

/// All 27 top-level Config sections that cli-config.yaml.example must document.
/// Derived from the typed Config struct (RESEARCH.md §Q2).
const REQUIRED_SECTIONS: &[&str] = &[
    "model",
    "agent",
    "terminal",
    "web",
    "gateway",
    "cron",
    "security",
    "rate_limit",
    "skills",
    "exec",
    "delegation",
    "batch",
    "memory",
    "compression",
    "providers",
    "custom_providers",
    "mcp_servers",
    "autonomous",
    "tools",
    "auxiliary",
    "browser",
    "extract",
    "prompt_caching",
    "kanban",
    "dashboard",
    "tts",
    "audio_cache",
];

/// REQ-37.1-02: cli-config.yaml.example must contain all 27 Config top-level section keys.
/// WAVE-0 RED: This test FAILS today because the example is missing mcp_servers, autonomous,
/// browser, extract, prompt_caching, kanban, dashboard, tts, audio_cache.
/// Turns GREEN in Plan 03.
#[test]
fn example_covers_all_typed_config_sections() {
    let example = load_example_yaml();
    let map = example
        .as_mapping()
        .expect("cli-config.yaml.example top level must be a YAML mapping");
    for section in REQUIRED_SECTIONS {
        assert!(
            map.contains_key(serde_yaml::Value::String(section.to_string())),
            "cli-config.yaml.example is missing section: `{section}` — \
             add a commented-default block for this Config field (REQ-37.1-02)"
        );
    }
}

/// REQ-37.1-02: The example file must deserialize cleanly to Config (no unknown-key errors,
/// no missing required fields).
/// WAVE-0 RED: This test FAILS today once the example gains new sections that the typed
/// Config does not yet have serde-unknown-field handling for, or if any required typed
/// field is missing.
/// Turns GREEN in Plan 03 (after both the example and any serde annotations are aligned).
#[test]
fn example_deserializes_to_config_without_errors() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../cli-config.yaml.example");
    let contents = std::fs::read_to_string(path).unwrap();
    let _: Config = serde_yaml::from_str(&contents)
        .expect("cli-config.yaml.example must deserialize cleanly to Config (REQ-37.1-02)");
}

/// REQ-37.1-02 (Open Question #3): The `kanban` key must exist at the top level.
/// Kanban coverage is KEY-presence only (not field-level — that is KanbanConfig's job).
/// WAVE-0 RED: This test FAILS today because cli-config.yaml.example has no `kanban:` section.
/// Turns GREEN in Plan 03.
#[test]
fn kanban_key_present() {
    let example = load_example_yaml();
    let map = example
        .as_mapping()
        .expect("cli-config.yaml.example top level must be a YAML mapping");
    assert!(
        map.contains_key(serde_yaml::Value::String("kanban".to_string())),
        "cli-config.yaml.example is missing top-level `kanban:` key (REQ-37.1-02, D-09)"
    );
}

// ---------------------------------------------------------------------------
// G3 GUARD (REQ-37.1-03): Private-range IP leak prevention
//
// cli-config.yaml.example is the seed source for install.sh's seed_templates().
// It is copied verbatim into every fresh ~/.ironhermes/config.yaml on install.
// Gap G3 was introduced when a developer LAN IP (127.0.0.1) was hard-coded
// in the ollama base_url example; D-11 (never-clobber on update) then preserved
// it in every affected user's config permanently.
//
// G3 was fixed in commit 85a135a9 (base_url now uses http://localhost:11434/v1).
// These tests ensure a future edit cannot silently reintroduce a private host.
// The scan reads RAW TEXT (not parsed YAML) so commented lines are also checked —
// a private IP parked in a comment is a leak waiting to be uncommented.
// ---------------------------------------------------------------------------

/// Returns true if `c` is a digit or a dot — used as a boundary check to ensure
/// we are not matching inside a larger number (e.g. to avoid matching `10.0.0.1`
/// when the preceding character is another digit making it part of `210.0.0.1`).
fn is_ip_char(c: char) -> bool {
    c.is_ascii_digit() || c == '.'
}

/// Scan `text` for RFC-1918 private-range IPv4 addresses and return the first
/// offending match (or None if the text is clean).
///
/// Matches:
///   - 10.x.x.x          (10.0.0.0/8) — requires all 4 octets
///   - 172.16.x.x–172.31.x.x (172.16.0.0/12)
///   - 192.168.x.x       (127.0.0.1/16)
///
/// Explicitly ALLOWED (returns None):
///   - 127.0.0.1 / localhost (loopback)
///   - bare port numbers like "11434" (no IP context)
///
/// The regex crate does not support look-around assertions, so boundary safety is
/// enforced by examining the character immediately before each candidate match.
fn first_private_ip_in(text: &str) -> Option<String> {
    use regex::Regex;

    // Match the candidate IPv4 dotted-quad prefixes that cover RFC-1918 ranges.
    // The regex itself captures the full address; we then check boundary and octet
    // range validity in Rust code (since look-around is unsupported in this crate).
    //
    // Pattern captures:
    //   192\.168\.[0-9]{1,3}\.[0-9]{1,3}                      → 192.168/16
    //   10\.[0-9]{1,3}\.[0-9]{1,3}\.[0-9]{1,3}               → 10/8 (all 4 octets required)
    //   172\.[0-9]{1,3}\.[0-9]{1,3}\.[0-9]{1,3}              → 172.x.x.x (range checked below)
    let re = Regex::new(r"(?:192\.168|10|172)\.[0-9]{1,3}\.[0-9]{1,3}(?:\.[0-9]{1,3})?")
        .expect("private-IP regex must compile");

    for m in re.find_iter(text) {
        let candidate = m.as_str();
        let start = m.start();

        // Boundary check: the character immediately before the match must NOT be
        // a digit or dot (otherwise we are inside a larger number, e.g. "2192.168").
        if start > 0 {
            let prev_char = text[..start].chars().next_back().unwrap();
            if is_ip_char(prev_char) {
                continue;
            }
        }

        // Parse all octets to validate the range and ensure we have 4 octets.
        let octets: Vec<u32> = candidate
            .split('.')
            .filter_map(|s| s.parse().ok())
            .collect();

        if octets.len() < 4 {
            // Incomplete address — not a valid IPv4; skip.
            continue;
        }

        let a = octets[0];
        let b = octets[1];

        let is_private = match a {
            192 if b == 168 => true,               // 127.0.0.1/16
            10 => true,                            // 10.0.0.0/8
            172 if (16..=31).contains(&b) => true, // 172.16.0.0/12
            _ => false,
        };

        if is_private {
            return Some(candidate.to_owned());
        }
    }

    None
}

/// REQ-37.1-03 (G3 guard): The shipped cli-config.yaml.example must contain no
/// RFC-1918 private-range IPv4 addresses — including in commented lines.
///
/// Reads the file as RAW TEXT so comments are scanned too. On failure the
/// message names the offending match so a future regression is immediately actionable.
#[test]
fn example_contains_no_private_range_ips() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../cli-config.yaml.example");
    let text = std::fs::read_to_string(path)
        .expect("cli-config.yaml.example must exist (G3 guard needs raw text)");

    if let Some(offender) = first_private_ip_in(&text) {
        panic!(
            "cli-config.yaml.example contains a private-range IP address: `{offender}`\n\
             This file is the seed source for install.sh — a private host leaks the \
             developer's LAN topology to every external installer (G3, REQ-37.1-03).\n\
             Replace the private IP with `localhost` or `127.0.0.1`."
        );
    }
}

/// REQ-37.1-03 (G3 guard, W3 anti-vacuous case): Prove that `first_private_ip_in`
/// actually fires on a real private IP — specifically the original G3 leak host
/// (127.0.0.1). If someone later neuters the regex or replaces it with a
/// stub, this test goes RED.
///
/// Also verifies negative controls:
///   - loopback 127.0.0.1 is allowed (returns None)
///   - a bare port number "port: 11434" has no IP → returns None (no false positives)
#[test]
fn detector_fires_on_synthetic_private_ip() {
    // Positive: a synthetic private IP must be detected. The original G3 leak
    // (REQ-37.1-03) was the developer LAN host 127.0.0.1; we assert on a
    // 172.16/12 address instead because the public-showcase PII scrub rewrites
    // every 192.168.x.x literal to loopback 127.0.0.1 — which the detector
    // correctly treats as ALLOWED — so a 192.168 literal would be a false
    // negative on the scrubbed branch. The 10.x and 172.x cases (here + below)
    // keep RFC1918 detection covered on both develop and the showcase branch.
    let synthetic = "providers:\n  ollama:\n    base_url: \"http://172.31.0.1:11434/v1\"\n";
    assert!(
        first_private_ip_in(synthetic).is_some(),
        "detector FAILED to fire on 172.31.0.1 — the G3 guard is vacuous (REQ-37.1-03)"
    );

    // Negative control 1: loopback is explicitly allowed.
    let loopback = "base_url: \"http://127.0.0.1:11434/v1\"\n";
    assert_eq!(
        first_private_ip_in(loopback),
        None,
        "detector must NOT flag 127.0.0.1 (loopback is allowed)"
    );

    // Negative control 2: a bare port number must not produce a false positive.
    let bare_port = "port: 11434\n";
    assert_eq!(
        first_private_ip_in(bare_port),
        None,
        "detector must NOT flag a bare port number with no IP context"
    );

    // Negative control 3: 10.x.x with only 3 octets (incomplete IP) — not a valid addr.
    // The regex requires all 4 octets for 10.x, so this should not match.
    let incomplete = "# see 10.0 networking docs\n";
    assert_eq!(
        first_private_ip_in(incomplete),
        None,
        "detector must NOT flag an incomplete dotted string that is not a full IPv4 address"
    );

    // Positive: other private ranges also fire.
    let ten_net = "host: \"http://10.0.1.5:8080\"\n";
    assert!(
        first_private_ip_in(ten_net).is_some(),
        "detector must fire on 10.x.x.x addresses (10.0.0.0/8)"
    );

    let v172 = "remote: \"http://172.20.1.1:11434\"\n";
    assert!(
        first_private_ip_in(v172).is_some(),
        "detector must fire on 172.16-31.x.x addresses (172.16.0.0/12)"
    );

    // Negative control 4: 172.x outside the /12 range must NOT match.
    let v172_out = "remote: \"http://172.32.1.1:11434\"\n";
    assert_eq!(
        first_private_ip_in(v172_out),
        None,
        "detector must NOT flag 172.32.x.x — outside the RFC-1918 172.16.0.0/12 range"
    );
}
