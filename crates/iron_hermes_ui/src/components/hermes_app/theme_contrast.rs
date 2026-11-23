//! Phase 54.4 Plan 12 (G-54.4-3, G-54.4-3b, D-02) — test-only contrast gate.
//!
//! The docked wheel and the config cards were ported from a dark-only
//! prototype and painted slate-dark literals, so they stayed dark on the light
//! themes. This module guards the fix by evaluating the SHIPPED CSS, not a copy
//! of it: it parses the five `[data-theme]` token blocks out of `site.css`,
//! applies the real `accent_inline_pair(theme, accent)` the way `theme_effects`
//! Effect 3 does (dark inks on dark themes, AA inks on light themes), resolves the wheel / card rules with a tiny `var()` / `color-mix()`
//! evaluator, and asserts WCAG contrast ratios plus pixel identity on slate-dark.
//!
//! Std only. The evaluator panics on any expression form it does not know: a
//! test evaluator must fail loudly, never guess.

use std::cell::Cell;
use std::collections::HashMap;

use crate::ui_prefs::AccentColor;

const SITE_CSS: &str = include_str!("../../../assets/site.css");
const WHEEL_CSS: &str = include_str!("../../../assets/wheel.css");
const SCREENS_CSS: &str = include_str!("../../../assets/screens.css");
const WHEEL_RS: &str = include_str!("wheel.rs");

// Plan 13 (G-54.4-4): the other two token systems and the consumer sheets.
const TOKENS_CSS: &str = include_str!("../../../assets/tokens.css");
const DESIGN_TOKENS_CSS: &str = include_str!("../../../assets/design-tokens.css");
const WARP_IH_CSS: &str = include_str!("../../../assets/warp-ih.css");
const BOTS_CSS: &str = include_str!("../../../assets/bots.css");
const COMPONENTS_CSS: &str = include_str!("../../../assets/components.css");
const DASHBOARD_CSS: &str = include_str!("../../../assets/dashboard.css");
const LOGS_CSS: &str = include_str!("../../../assets/logs.css");
const KANBAN_CSS: &str = include_str!("../../../assets/kanban.css");
const VOICE_CSS: &str = include_str!("../../../assets/voice.css");
const TOOLS_CSS: &str = include_str!("../../../assets/tools.css");
const ORB_JS: &str = include_str!("../../../assets/orb.js");
const AVATAR_JS: &str = include_str!("../../../assets/avatar.js");
const APP_RS: &str = include_str!("../../app.rs");

/// The four sheets that declare custom properties on `<html>`, in the order
/// `app.rs` links them. Pinned by `html_token_sheets_follow_app_link_order`.
const HTML_TOKEN_SHEETS: [(&str, &str); 4] = [
    ("tokens.css", TOKENS_CSS),
    ("design-tokens.css", DESIGN_TOKENS_CSS),
    ("warp-ih.css", WARP_IH_CSS),
    ("site.css", SITE_CSS),
];

const THEMES: [&str; 5] = [
    "slate-dark",
    "slate-light",
    "iron-dark",
    "terminal-dark",
    "parchment-light",
];
const DARK_THEMES: [&str; 3] = ["slate-dark", "iron-dark", "terminal-dark"];
const ACCENTS: [AccentColor; 5] = [
    AccentColor::Teal,
    AccentColor::Orange,
    AccentColor::Green,
    AccentColor::Violet,
    AccentColor::Amber,
];
const REQUIRED_TOKENS: [&str; 8] = [
    "--bg",
    "--bg-card",
    "--bg-card-hover",
    "--bg-deep",
    "--text",
    "--text-dim",
    "--gray",
    "--amber",
];

/// Channels 0-255, alpha 0-1.
type Rgba = [f64; 4];

// ---------------------------------------------------------------------------
// CSS source helpers
// ---------------------------------------------------------------------------

/// Remove `/* ... */` comments so prose can neither satisfy nor trip a check.
fn strip_css_comments(css: &str) -> String {
    let mut out = String::with_capacity(css.len());
    let mut rest = css;
    while let Some(start) = rest.find("/*") {
        out.push_str(&rest[..start]);
        match rest[start + 2..].find("*/") {
            Some(end) => rest = &rest[start + 2 + end + 2..],
            None => rest = "",
        }
    }
    out.push_str(rest);
    out
}

/// One parsed rule: its selector items (whitespace-collapsed) and declarations.
struct Rule {
    selectors: Vec<String>,
    decls: Vec<(String, String)>,
}

/// Parse every `selector { decls }` pair in source order. Nested at-rule
/// wrappers are tolerated: only the text after the innermost opening brace is
/// the selector, so a keyframe step or a media-nested rule parses as a rule.
fn parse_rules(css: &str) -> Vec<Rule> {
    let css = strip_css_comments(css);
    let mut rules = Vec::new();
    for chunk in css.split('}') {
        let Some((sel_text, body)) = chunk.rsplit_once('{') else {
            continue;
        };
        let sel_text = sel_text.rsplit_once('{').map_or(sel_text, |(_, s)| s);
        let selectors = sel_text
            .split(',')
            .map(|s| s.split_whitespace().collect::<Vec<_>>().join(" "))
            .filter(|s| !s.is_empty())
            .collect();
        let decls = body
            .split(';')
            .filter_map(|d| {
                let (name, value) = d.split_once(':')?;
                Some((name.trim().to_string(), value.trim().to_string()))
            })
            .collect();
        rules.push(Rule { selectors, decls });
    }
    rules
}

/// Every value of `property` declared by a rule whose selector list contains
/// EXACTLY `selector_item`, in source order. Callers use the last one (cascade
/// order for equal specificity).
fn rule_decls(css: &str, selector_item: &str, property: &str) -> Vec<String> {
    let mut out = Vec::new();
    for rule in parse_rules(css) {
        if rule.selectors.iter().any(|s| s == selector_item) {
            for (name, value) in &rule.decls {
                if name == property {
                    out.push(value.clone());
                }
            }
        }
    }
    out
}

/// The last declared value, panicking with the selector and property when the
/// rule is absent (a missing rule is the failure this gate exists to catch).
fn last_decl(css: &str, selector_item: &str, property: &str) -> String {
    rule_decls(css, selector_item, property)
        .pop()
        .unwrap_or_else(|| panic!("no `{property}` declaration for selector `{selector_item}`"))
}

// ---------------------------------------------------------------------------
// Colour model + evaluator
// ---------------------------------------------------------------------------

fn parse_hex(s: &str) -> Option<Rgba> {
    let h = s.strip_prefix('#')?;
    if h.len() != 6 {
        return None;
    }
    let c = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok().map(f64::from);
    Some([c(0)?, c(2)?, c(4)?, 1.0])
}

fn parse_rgb_fn(s: &str) -> Option<Rgba> {
    let inner = s
        .strip_prefix("rgba(")
        .or_else(|| s.strip_prefix("rgb("))?
        .strip_suffix(')')?;
    let parts: Vec<f64> = inner
        .split(',')
        .map(|p| p.trim().parse::<f64>())
        .collect::<Result<_, _>>()
        .ok()?;
    match parts.as_slice() {
        [r, g, b] => Some([*r, *g, *b, 1.0]),
        [r, g, b, a] => Some([*r, *g, *b, *a]),
        _ => None,
    }
}

/// The five `[data-theme="..."] { ... }` blocks of `site.css`, as colour tokens.
fn theme_tokens(theme: &str) -> HashMap<String, Rgba> {
    let selector = format!("[data-theme=\"{theme}\"]");
    let rule = parse_rules(SITE_CSS)
        .into_iter()
        .find(|r| r.selectors.contains(&selector))
        .unwrap_or_else(|| panic!("site.css has no `{selector}` token block"));
    let mut tokens = HashMap::new();
    for (name, value) in rule.decls {
        if let Some(c) = parse_hex(&value).or_else(|| parse_rgb_fn(&value)) {
            tokens.insert(name, c);
        }
    }
    for key in REQUIRED_TOKENS {
        assert!(
            tokens.contains_key(key),
            "theme `{theme}` block is missing required token `{key}`"
        );
    }
    tokens
}

/// The custom properties `<html>` carries for `theme`, as raw expressions.
///
/// Walks the four token sheets in the order `app.rs` links them. A rule applies
/// when one of its selector items is exactly `:root` or exactly
/// `[data-theme="{theme}"]`. Both match `<html>` at specificity 0,1,0, so source
/// order decides, exactly as in the browser: a later insert wins. This is what
/// reproduces the link-order clobber behind G-54.4-4 (design-tokens.css, linked
/// after tokens.css, replaces tokens.css's themed `--fg`).
fn html_custom_props(theme: &str) -> HashMap<String, String> {
    let theme_item = format!("[data-theme=\"{theme}\"]");
    let mut props = HashMap::new();
    for (_, css) in HTML_TOKEN_SHEETS {
        for rule in parse_rules(css) {
            if rule
                .selectors
                .iter()
                .any(|s| s == ":root" || *s == theme_item)
            {
                for (name, value) in rule.decls {
                    if name.starts_with("--") {
                        props.insert(name, value);
                    }
                }
            }
        }
    }
    props
}

/// Evaluation context for one (theme, accent) pair.
struct Ctx {
    tokens: HashMap<String, Rgba>,
    /// Wheel-local `--wheel-*` declarations as raw expressions, resolved lazily
    /// so a wheel variable may reference another one.
    wheel: HashMap<String, String>,
    /// The `<html>` custom-property cascade of the four token sheets (raw
    /// expressions). Consulted after `tokens`; see `Ctx::var`.
    html: HashMap<String, String>,
    /// Recursion depth of `var()` resolution, to turn a cycle into a panic.
    depth: Cell<u32>,
}

impl Ctx {
    fn new(theme: &str, accent: AccentColor) -> Ctx {
        let mut tokens = theme_tokens(theme);
        // theme_effects Effect 3 writes the accent inline on <html> for every
        // theme, so it beats the [data-theme] block. The model applies
        // `accent_inline_pair` (the very function Effect 3 calls), so it can
        // never drift from what the browser receives (G-54.4-18).
        let (base, bright) =
            crate::components::hermes_app::theme_effects::accent_inline_pair(theme, accent);
        tokens.insert("--teal".into(), parse_hex(base).expect("accent base hex"));
        tokens.insert(
            "--teal-bright".into(),
            parse_hex(bright).expect("accent hover hex"),
        );
        let mut wheel = HashMap::new();
        let theme_item = format!("[data-theme=\"{theme}\"] .wheel-shell");
        // Base `.wheel-shell` first, then the per-theme override (higher
        // specificity, so it wins regardless of source order).
        for item in [".wheel-shell".to_string(), theme_item] {
            for rule in parse_rules(WHEEL_CSS) {
                if rule.selectors.contains(&item) {
                    for (name, value) in rule.decls {
                        if name.starts_with("--wheel-") {
                            wheel.insert(name, value);
                        }
                    }
                }
            }
        }
        Ctx {
            tokens,
            wheel,
            html: html_custom_props(theme),
            depth: Cell::new(0),
        }
    }

    /// Lookup order: wheel-local, then `tokens` (the site.css theme block plus
    /// the inline accent), then the `<html>` cascade.
    ///
    /// `tokens` before `html` is correct only while the light family-3 block
    /// declares no site.css legacy name; `html_cascade_agrees_with_site_theme_tokens`
    /// enforces that.
    fn var(&self, name: &str) -> Rgba {
        if let Some(expr) = self.wheel.get(name) {
            return eval(expr, self);
        }
        if let Some(c) = self.tokens.get(name) {
            return *c;
        }
        let expr = self
            .html
            .get(name)
            .unwrap_or_else(|| panic!("unresolved custom property `{name}`"));
        let depth = self.depth.get() + 1;
        assert!(
            depth <= 32,
            "custom property `{name}` recurses more than 32 levels deep (cycle?)"
        );
        self.depth.set(depth);
        let value = eval(expr, self);
        self.depth.set(depth - 1);
        value
    }
}

/// Evaluate `name` straight from the `<html>` cascade, bypassing `tokens`.
fn html_value(ctx: &Ctx, name: &str) -> Rgba {
    let expr = ctx
        .html
        .get(name)
        .unwrap_or_else(|| panic!("`{name}` is not declared on <html> by any token sheet"));
    eval(expr, ctx)
}

/// The last `background` of `selector`, evaluated and composited over an
/// OPAQUE `base`. The composite helper every plan-13 surface test uses.
fn surface(css: &str, selector: &str, base: Rgba, ctx: &Ctx) -> Rgba {
    over(decl_color(css, selector, "background", ctx), base)
}

/// Split on top-level commas only (parentheses nest).
fn split_top_level(s: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0_i32;
    let mut start = 0;
    for (i, ch) in s.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth == 0 => {
                parts.push(s[start..i].trim());
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(s[start..].trim());
    parts
}

/// CSS Color 4 OKLCH -> sRGB, opaque. `l` is 0..1, `h` in degrees. Linear
/// channels are clamped to [0, 1] (gamut clip), then sRGB-encoded.
fn oklch_to_srgb(l: f64, c: f64, h: f64) -> Rgba {
    let (a, b) = (c * h.to_radians().cos(), c * h.to_radians().sin());
    let l_ = (l + 0.396_337_777_4 * a + 0.215_803_757_3 * b).powi(3);
    let m_ = (l - 0.105_561_345_8 * a - 0.063_854_172_8 * b).powi(3);
    let s_ = (l - 0.089_484_177_5 * a - 1.291_485_548_0 * b).powi(3);
    let linear = [
        4.076_741_662_1 * l_ - 3.307_711_591_3 * m_ + 0.230_969_929_2 * s_,
        -1.268_438_004_6 * l_ + 2.609_757_401_1 * m_ - 0.341_319_396_5 * s_,
        -0.004_196_086_3 * l_ - 0.703_418_614_7 * m_ + 1.707_614_701_0 * s_,
    ];
    let encode = |x: f64| {
        let x = x.clamp(0.0, 1.0);
        let v = if x < 0.003_130_8 {
            12.92 * x
        } else {
            1.055 * x.powf(1.0 / 2.4) - 0.055
        };
        v * 255.0
    };
    [encode(linear[0]), encode(linear[1]), encode(linear[2]), 1.0]
}

/// `oklch(<n>% <n> <n>)` exactly: no alpha, no `none`. Anything else panics.
fn parse_oklch(s: &str) -> Option<Rgba> {
    let inner = s.strip_prefix("oklch(")?.strip_suffix(')')?;
    let parts: Vec<&str> = inner.split_whitespace().collect();
    let [l, c, h] = parts.as_slice() else {
        panic!("oklch() form not understood (need `L% C H`): `{s}`");
    };
    let l: f64 = l
        .strip_suffix('%')
        .and_then(|n| n.parse().ok())
        .unwrap_or_else(|| panic!("oklch() lightness must be a percentage: `{s}`"));
    let c: f64 = c
        .parse()
        .unwrap_or_else(|_| panic!("oklch() chroma not a number: `{s}`"));
    let h: f64 = h
        .parse()
        .unwrap_or_else(|_| panic!("oklch() hue not a number: `{s}`"));
    Some(oklch_to_srgb(l / 100.0, c, h))
}

/// Evaluate exactly: `var(--x)`, `transparent`, `#rrggbb`, `rgb[a](...)`,
/// `oklch(<n>% <n> <n>)` and `color-mix(in srgb, A P%, B)`. Anything else
/// panics with the expression.
fn eval(expr: &str, ctx: &Ctx) -> Rgba {
    let e = expr.trim();
    if e == "transparent" {
        return [0.0, 0.0, 0.0, 0.0];
    }
    if let Some(c) = parse_hex(e)
        .or_else(|| parse_rgb_fn(e))
        .or_else(|| parse_oklch(e))
    {
        return c;
    }
    if let Some(inner) = e.strip_prefix("var(").and_then(|r| r.strip_suffix(')')) {
        let name = inner.trim();
        if name.starts_with("--") && !name.contains(',') && !name.contains('(') {
            return ctx.var(name);
        }
    }
    if let Some(inner) = e
        .strip_prefix("color-mix(")
        .and_then(|r| r.strip_suffix(')'))
    {
        let parts = split_top_level(inner);
        if parts.len() == 3 && parts[0] == "in srgb" {
            let (a_expr, pct) = parts[1]
                .rsplit_once(char::is_whitespace)
                .unwrap_or_else(|| panic!("color-mix operand without a percentage: `{e}`"));
            let p: f64 = pct
                .trim()
                .strip_suffix('%')
                .and_then(|n| n.parse().ok())
                .unwrap_or_else(|| panic!("color-mix percentage not understood: `{e}`"));
            let a = eval(a_expr, ctx);
            if parts[2] == "transparent" {
                return [a[0], a[1], a[2], a[3] * p / 100.0];
            }
            let b = eval(parts[2], ctx);
            assert!(
                (a[3] - 1.0).abs() < 1e-9 && (b[3] - 1.0).abs() < 1e-9,
                "color-mix of translucent colours is not supported: `{e}`"
            );
            let f = p / 100.0;
            return [
                a[0] * f + b[0] * (1.0 - f),
                a[1] * f + b[1] * (1.0 - f),
                a[2] * f + b[2] * (1.0 - f),
                1.0,
            ];
        }
    }
    panic!("theme_contrast evaluator does not understand `{expr}`");
}

/// The first top-level token of a `background` shorthand: a balanced
/// `var(...)` / `color-mix(...)` call, or a `#hex`. Anything else panics.
fn bg_colour(value: &str) -> String {
    let v = value.trim();
    if v.starts_with("var(") || v.starts_with("color-mix(") {
        let mut depth = 0_i32;
        for (i, ch) in v.char_indices() {
            match ch {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        return v[..=i].to_string();
                    }
                }
                _ => {}
            }
        }
        panic!("unbalanced parentheses in background `{value}`");
    }
    if v.starts_with('#') {
        return v.split_whitespace().next().unwrap_or(v).to_string();
    }
    panic!("background shorthand does not start with a colour token we understand: `{value}`");
}

/// Resolve `property` of the last rule with exactly this selector item.
fn decl_color(css: &str, selector_item: &str, property: &str, ctx: &Ctx) -> Rgba {
    eval(&last_decl(css, selector_item, property), ctx)
}

// ---------------------------------------------------------------------------
// WCAG helpers
// ---------------------------------------------------------------------------

fn lin(channel: f64) -> f64 {
    let c = channel / 255.0;
    if c <= 0.03928 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn luminance(c: Rgba) -> f64 {
    0.2126 * lin(c[0]) + 0.7152 * lin(c[1]) + 0.0722 * lin(c[2])
}

fn contrast(a: Rgba, b: Rgba) -> f64 {
    let (la, lb) = (luminance(a), luminance(b));
    let (hi, lo) = if la >= lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

/// Alpha-composite `fg` over an OPAQUE `bg`; the result is opaque.
fn over(fg: Rgba, bg: Rgba) -> Rgba {
    assert!(
        (bg[3] - 1.0).abs() < 1e-9,
        "over() needs an opaque background, got alpha {}",
        bg[3]
    );
    let a = fg[3];
    [
        fg[0] * a + bg[0] * (1.0 - a),
        fg[1] * a + bg[1] * (1.0 - a),
        fg[2] * a + bg[2] * (1.0 - a),
        1.0,
    ]
}

// ---------------------------------------------------------------------------
// wheel.rs source helpers
// ---------------------------------------------------------------------------

/// Non-test code of `wheel.rs` with `//` comment lines dropped.
fn wheel_render_code() -> String {
    // The attribute must start a line: a doc comment may mention it in prose.
    let code = WHEEL_RS
        .split("\n#[cfg(test)]\n")
        .next()
        .expect("wheel.rs has a non-test region");
    code.lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn is_dark(theme: &str) -> bool {
    DARK_THEMES.contains(&theme)
}

// ---------------------------------------------------------------------------
// Task 1 — inactive wheel labels are legible on every theme
// ---------------------------------------------------------------------------

#[test]
fn theme_tokens_parse_for_all_five_themes() {
    for theme in THEMES {
        let tokens = theme_tokens(theme);
        for key in REQUIRED_TOKENS {
            assert!(tokens.contains_key(key), "{theme}: missing {key}");
        }
    }
}

#[test]
fn wheel_labels_meet_aa_on_every_theme() {
    let mut min_all = f64::MAX;
    let mut min_dark = f64::MAX;
    for theme in THEMES {
        for accent in ACCENTS {
            let ctx = Ctx::new(theme, accent);
            let backing = decl_color(WHEEL_CSS, ".wheel-backing", "fill", &ctx);
            let wedge = decl_color(WHEEL_CSS, ".wheel-wedge", "fill", &ctx);
            let surface = over(wedge, over(backing, [0.0, 0.0, 0.0, 1.0]));
            let label = over(decl_color(WHEEL_CSS, ".wheel-label", "fill", &ctx), surface);
            let ratio = contrast(label, surface);
            assert!(
                ratio >= 4.5,
                "{theme}/{accent:?}: inactive wheel label {ratio:.2}:1 is below 4.5:1"
            );
            if is_dark(theme) {
                assert!(
                    ratio >= 6.8,
                    "{theme}/{accent:?}: dark-theme label {ratio:.2}:1 is below 6.8:1"
                );
                min_dark = min_dark.min(ratio);
            }
            min_all = min_all.min(ratio);
        }
    }
    println!("measured: inactive label min {min_all:.2}:1 (all), {min_dark:.2}:1 (dark)");
}

#[test]
fn wheel_paints_on_an_opaque_backing_disc() {
    for theme in THEMES {
        let ctx = Ctx::new(theme, AccentColor::Teal);
        let backing = decl_color(WHEEL_CSS, ".wheel-backing", "fill", &ctx);
        assert!(
            (backing[3] - 1.0).abs() < 1e-9,
            "{theme}: .wheel-backing fill alpha is {} (must be 1.0)",
            backing[3]
        );
    }
    let code = wheel_render_code();
    let backing_at = code
        .find("class: \"wheel-backing\"")
        .expect("wheel.rs must render a `wheel-backing` circle");
    let first_rim = code
        .find("rim-ring")
        .expect("wheel.rs must still render the rim rings");
    assert!(
        backing_at < first_rim,
        "the backing disc must be declared before the first rim-ring"
    );
}

#[test]
fn wheel_label_carries_no_inline_fill() {
    // Assembled from fragments so this test cannot match its own source.
    let needle = format!("#{}{}", "9aa4", "ad");
    let code = wheel_render_code().to_lowercase();
    assert!(
        !code.contains(&needle),
        "wheel.rs still carries the inline slate label colour: it must come from wheel.css"
    );
}

// ---------------------------------------------------------------------------
// Task 2 — every remaining wheel colour follows the theme and the accent
// ---------------------------------------------------------------------------

const LIGHT_THEMES: [&str; 2] = ["slate-light", "parchment-light"];

/// Rounded channels and `round(alpha * 100)`, so comparisons are integer.
fn q(c: Rgba) -> [i64; 4] {
    [
        c[0].round() as i64,
        c[1].round() as i64,
        c[2].round() as i64,
        (c[3] * 100.0).round() as i64,
    ]
}

/// The wedge surface a label sits on: the inactive wedge fill over the
/// opaque backing disc.
fn inactive_wedge_surface(ctx: &Ctx) -> Rgba {
    let backing = decl_color(WHEEL_CSS, ".wheel-backing", "fill", ctx);
    over(
        decl_color(WHEEL_CSS, ".wheel-wedge", "fill", ctx),
        over(backing, [0.0, 0.0, 0.0, 1.0]),
    )
}

/// The active wedge surface: the active fill over the backing disc.
fn active_wedge_surface(ctx: &Ctx) -> Rgba {
    let backing = decl_color(WHEEL_CSS, ".wheel-backing", "fill", ctx);
    over(
        decl_color(WHEEL_CSS, ".wheel-wedge.is-active", "fill", ctx),
        over(backing, [0.0, 0.0, 0.0, 1.0]),
    )
}

#[test]
fn wheel_active_labels_meet_aa_for_every_accent() {
    let mut min_label = f64::MAX;
    let mut min_glyph = f64::MAX;
    for theme in THEMES {
        for accent in ACCENTS {
            let ctx = Ctx::new(theme, accent);
            let surface = active_wedge_surface(&ctx);
            for (selector, min) in [
                (".wheel-label.is-active", &mut min_label),
                (".wheel-glyph.is-active", &mut min_glyph),
            ] {
                let ink = over(decl_color(WHEEL_CSS, selector, "fill", &ctx), surface);
                let ratio = contrast(ink, surface);
                assert!(
                    ratio >= 4.5,
                    "{theme}/{accent:?}: `{selector}` {ratio:.2}:1 is below 4.5:1"
                );
                *min = min.min(ratio);
            }
        }
    }
    println!("measured: active label min {min_label:.2}:1, active glyph min {min_glyph:.2}:1");
}

#[test]
fn wheel_light_theme_glyphs_and_hub_stay_legible() {
    let mut min_glyph = f64::MAX;
    let mut min_hub = f64::MAX;
    for theme in LIGHT_THEMES {
        for accent in ACCENTS {
            let ctx = Ctx::new(theme, accent);
            let surface = inactive_wedge_surface(&ctx);
            let glyph = over(decl_color(WHEEL_CSS, ".wheel-glyph", "fill", &ctx), surface);
            let ratio = contrast(glyph, surface);
            assert!(
                ratio >= 3.0,
                "{theme}/{accent:?}: wedge glyph {ratio:.2}:1 is below 3.0:1"
            );
            min_glyph = min_glyph.min(ratio);
            for stop_sel in [".hub-stop--hi", ".hub-stop--mid", ".hub-stop--lo"] {
                let stop = decl_color(WHEEL_CSS, stop_sel, "stop-color", &ctx);
                assert!(
                    (stop[3] - 1.0).abs() < 1e-9,
                    "{theme}: `{stop_sel}` must be opaque"
                );
                for hub_sel in ["#hub-label", "#hub-sub", "#hub-cta", "#hub-glyph"] {
                    let ink = over(decl_color(WHEEL_CSS, hub_sel, "fill", &ctx), stop);
                    let ratio = contrast(ink, stop);
                    assert!(
                        ratio >= 4.5,
                        "{theme}/{accent:?}: `{hub_sel}` on `{stop_sel}` {ratio:.2}:1 is below 4.5:1"
                    );
                    min_hub = min_hub.min(ratio);
                }
            }
        }
    }
    println!("measured: light glyph min {min_glyph:.2}:1, light hub text min {min_hub:.2}:1");
}

#[test]
fn wheel_light_override_matches_light_themes() {
    let rules = parse_rules(WHEEL_CSS);
    for theme in THEMES {
        let item = format!("[data-theme=\"{theme}\"] .wheel-shell");
        let has_override = rules.iter().any(|r| {
            r.selectors.contains(&item) && r.decls.iter().any(|(n, _)| n == "--wheel-accent")
        });
        let bg_luminance = luminance(theme_tokens(theme)["--bg"]);
        let is_light = bg_luminance > 0.5;
        assert_eq!(
            has_override, is_light,
            "{theme}: --bg luminance {bg_luminance:.3} (light = {is_light}) but wheel.css \
             light override present = {has_override}"
        );
    }
}

#[test]
fn wheel_slate_dark_default_accent_is_pixel_identical() {
    let ctx = Ctx::new("slate-dark", AccentColor::Teal);
    let expected: [(&str, &str, [i64; 4]); 22] = [
        (".wheel-label", "fill", [154, 164, 173, 100]),
        (".wheel-label.is-active", "fill", [86, 212, 221, 100]),
        (".wheel-glyph.is-active", "fill", [86, 212, 221, 100]),
        ("#hub-glyph", "fill", [86, 212, 221, 100]),
        (".wheel-glyph", "fill", [57, 197, 207, 65]),
        (".wheel-wedge", "stroke", [57, 197, 207, 10]),
        (".wheel-wedge.is-active", "fill", [57, 197, 207, 22]),
        (".wheel-wedge.is-active", "stroke", [57, 197, 207, 70]),
        (".wheel-tick--major", "stroke", [57, 197, 207, 55]),
        (".wheel-tick--minor", "stroke", [110, 118, 129, 35]),
        (".wheel-sep", "stroke", [57, 197, 207, 22]),
        (".wheel-inner-ring", "stroke", [57, 197, 207, 22]),
        (".rim-ring--outer", "stroke", [57, 197, 207, 20]),
        (".rim-ring--inner", "stroke", [57, 197, 207, 12]),
        ("#hub-label", "fill", [230, 237, 243, 100]),
        ("#hub-sub", "fill", [110, 118, 129, 100]),
        ("#hub-cta", "fill", [57, 197, 207, 100]),
        ("#hub-arrow", "fill", [57, 197, 207, 100]),
        ("#hub-bg", "stroke", [57, 197, 207, 55]),
        (".hub-ring--inner", "stroke", [57, 197, 207, 18]),
        (".hub-ring--dashed", "stroke", [57, 197, 207, 10]),
        ("#hub-aura", "fill", [0, 0, 0, 0]),
    ];
    for (selector, property, want) in expected {
        let got = q(decl_color(WHEEL_CSS, selector, property, &ctx));
        assert_eq!(
            got, want,
            "slate-dark/Teal: `{selector}` {property} drifted from its pre-fix value"
        );
    }
    let card = ctx.var("--bg-card");
    let wedge = decl_color(WHEEL_CSS, ".wheel-wedge", "fill", &ctx);
    let composite = q(over(wedge, card));
    assert_eq!(
        composite,
        [22, 27, 34, 100],
        "the wedge fill over the slate card surface must stay the slate card triple"
    );
}

#[test]
fn wheel_rs_render_code_has_no_colour_literals() {
    let code = wheel_render_code();
    let mut none_seen = 0;
    for (at, _) in code.match_indices("fill: \"") {
        let rest = &code[at + "fill: \"".len()..];
        let value = rest.split('"').next().expect("closing quote");
        assert!(
            value == "none" || value == "url(#hub-grad)",
            "wheel.rs render code carries an inline fill `{value}`: move it to wheel.css"
        );
        if value == "none" {
            none_seen += 1;
        }
    }
    assert!(
        none_seen >= 3,
        "positive anchor: expected several `fill: \"none\"`, saw {none_seen}"
    );
    assert_eq!(
        code.matches("stroke: \"").count(),
        0,
        "wheel.rs render code carries an inline stroke colour"
    );
    assert_eq!(
        code.matches("\"stop-color\"").count(),
        0,
        "wheel.rs render code carries an inline stop-color"
    );
}

#[test]
fn wheel_css_wheel_region_has_no_colour_literals() {
    let css = strip_css_comments(WHEEL_CSS);
    let end = css
        .find(".stub-overlay")
        .expect("wheel.css must still contain the .stub-overlay section");
    let region = &css[..end];
    assert!(region.contains(".wheel-wedge"), "positive anchor missing");
    assert!(region.contains("#hub-cta"), "positive anchor missing");
    for needle in ["rgba(", "rgb("] {
        for (at, _) in region.match_indices(needle) {
            let tail: String = region[at..]
                .chars()
                .filter(|c| !c.is_whitespace())
                .collect();
            assert!(
                tail.starts_with("rgba(0,0,0,"),
                "colour function literal in the wheel region: `{}`",
                region[at..].chars().take(40).collect::<String>()
            );
        }
    }
    let bytes = region.as_bytes();
    for (at, _) in region.match_indices('#') {
        let run = bytes[at + 1..]
            .iter()
            .take_while(|b| b.is_ascii_hexdigit())
            .count();
        let next = bytes.get(at + 1 + run).copied();
        let ends_name = next.is_none_or(|b| !(b.is_ascii_alphanumeric() || b == b'_' || b == b'-'));
        assert!(
            !((3..=8).contains(&run) && ends_name),
            "hex colour literal in the wheel region: `{}`",
            region[at..].chars().take(12).collect::<String>()
        );
    }
}

// ---------------------------------------------------------------------------
// Task 3 — cards, panels and sibling surfaces follow the theme (G-54.4-3b)
// ---------------------------------------------------------------------------

/// The pre-fix slate card surface at the `.card` alpha, as numbers (this is a
/// test fixture, not CSS text, so it cannot trip the literal-sweep test).
const PRE_FIX_SLATE_CARD: Rgba = [22.0, 27.0, 34.0, 0.65];

/// A surface background composited over the theme's page background.
fn surface_over_page(css: &str, selector: &str, ctx: &Ctx) -> Rgba {
    over(
        decl_color(css, selector, "background", ctx),
        ctx.var("--bg"),
    )
}

#[test]
fn cards_and_panels_meet_aa_on_every_theme() {
    let mut min_text = f64::MAX;
    let mut min_dim = f64::MAX;
    for theme in THEMES {
        let ctx = Ctx::new(theme, AccentColor::Teal);
        for selector in [".card", ".panel"] {
            let surface = surface_over_page(SCREENS_CSS, selector, &ctx);
            let text = contrast(ctx.var("--text"), surface);
            let dim = contrast(ctx.var("--text-dim"), surface);
            assert!(
                text >= 4.5,
                "{theme}: --text on `{selector}` {text:.2}:1 is below 4.5:1"
            );
            assert!(
                dim >= 4.5,
                "{theme}: --text-dim on `{selector}` {dim:.2}:1 is below 4.5:1"
            );
            min_text = min_text.min(text);
            min_dim = min_dim.min(dim);
        }
    }
    println!("measured: --text on card/panel min {min_text:.2}:1, --text-dim min {min_dim:.2}:1");
}

#[test]
fn swept_surfaces_identical_on_slate_dark() {
    let ctx = Ctx::new("slate-dark", AccentColor::Teal);
    let card_65: [i64; 4] = [22, 27, 34, 65];
    let card_55: [i64; 4] = [22, 27, 34, 55];
    let screens: [(&str, [i64; 4]); 10] = [
        (".card", card_65),
        (".panel", card_65),
        (".tool-card", card_65),
        (".plat-card", card_65),
        (".gw-scope-select", card_65),
        (".model-popup", card_65),
        (".stat-card", card_65),
        (".row", card_55),
        (".soul-preview", card_55),
        (".sched-row", card_55),
    ];
    for (selector, want) in screens {
        let got = q(decl_color(SCREENS_CSS, selector, "background", &ctx));
        assert_eq!(got, want, "slate-dark: `{selector}` background drifted");
    }
    for (selector, want) in [
        (".statbar", [13, 17, 23, 65]),
        (".app-footer", [13, 17, 23, 85]),
    ] {
        let got = q(decl_color(SITE_CSS, selector, "background", &ctx));
        assert_eq!(got, want, "slate-dark: `{selector}` background drifted");
    }
    let tooltip = q(decl_color(WHEEL_CSS, ".wheel-tooltip", "background", &ctx));
    assert_eq!(
        tooltip,
        [13, 17, 23, 95],
        "slate-dark: `.wheel-tooltip` background drifted"
    );
}

#[test]
fn dark_cards_keep_or_raise_text_contrast() {
    for theme in ["iron-dark", "terminal-dark"] {
        let ctx = Ctx::new(theme, AccentColor::Teal);
        let page = ctx.var("--bg");
        let text = ctx.var("--text");
        let new_card = surface_over_page(SCREENS_CSS, ".card", &ctx);
        let old_card = over(PRE_FIX_SLATE_CARD, page);
        let (now, before) = (contrast(text, new_card), contrast(text, old_card));
        println!("measured: {theme} --text on .card {now:.2}:1 (pre-fix {before:.2}:1)");
        assert!(
            now >= before,
            "{theme}: --text on .card fell from {before:.2}:1 to {now:.2}:1"
        );
    }
}

#[test]
fn no_slate_surface_literals_remain() {
    // Assembled at runtime so this test cannot match its own source.
    let card = format!("rgba({},{},{},", "22", "27", "34");
    let page = format!("rgba({},{},{},", "13", "17", "23");
    let hover = format!("rgba({},{},{},", "28", "33", "41");
    for (name, css) in [
        ("screens.css", SCREENS_CSS),
        ("site.css", SITE_CSS),
        ("wheel.css", WHEEL_CSS),
        ("kanban.css", KANBAN_CSS),
        ("bots.css", BOTS_CSS),
        ("voice.css", VOICE_CSS),
        ("components.css", COMPONENTS_CSS),
        ("dashboard.css", DASHBOARD_CSS),
    ] {
        let flat: String = strip_css_comments(css)
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        assert!(
            !css.is_empty() && flat.contains('{'),
            "{name}: positive anchor failed (empty or unparsable sheet)"
        );
        assert!(
            !flat.contains(&card),
            "{name} still paints the slate card surface as a literal"
        );
        assert!(
            !flat.contains(&page),
            "{name} still paints the slate page surface as a literal"
        );
        assert!(
            !flat.contains(&hover),
            "{name} still paints the slate card-hover surface as a literal"
        );
    }
}

/// The three W2 hover rules in screens.css as (selector, alpha percent). The
/// slate card-hover triple at that alpha is the pre-fix background.
const HOVER_RULES: [(&str, i64); 3] = [
    (".card:hover", 75),
    (".row:hover", 85),
    (".model-popup-row:hover", 85),
];

/// The surface a hover background sits on: the page for `.card` / `.row`, the
/// `.model-popup` surface (itself over the page) for a popup row.
fn hover_base(selector: &str, ctx: &Ctx) -> Rgba {
    let page = ctx.var("--bg");
    if selector == ".model-popup-row:hover" {
        surface(SCREENS_CSS, ".model-popup", page, ctx)
    } else {
        page
    }
}

#[test]
fn hover_states_are_light_and_legible_on_light_themes() {
    let mut mins = Mins::default();
    for theme in LIGHT_THEMES {
        for accent in ACCENTS {
            let who = format!("{theme}/{accent:?}");
            let ctx = Ctx::new(theme, accent);
            for (selector, _) in HOVER_RULES {
                let surf = surface(SCREENS_CSS, selector, hover_base(selector, &ctx), &ctx);
                assert_light(selector, &who, surf, &mut mins);
                for (token, label) in [("--text", "text"), ("--text-dim", "dim")] {
                    let ratio = ink_ratio(ctx.var(token), surf);
                    assert!(
                        ratio >= 4.5,
                        "{who}: `{token}` on `{selector}` {ratio:.2}:1 is below 4.5:1"
                    );
                    mins.note(&format!("{selector} {label}"), ratio);
                }
            }
        }
    }
    mins.report("hover");
}

#[test]
fn hover_states_identical_on_slate_dark() {
    let ctx = Ctx::new("slate-dark", AccentColor::Teal);
    for (selector, alpha) in HOVER_RULES {
        let got = q(decl_color(SCREENS_CSS, selector, "background", &ctx));
        assert_eq!(
            got,
            [28, 33, 41, alpha],
            "slate-dark: `{selector}` background drifted from its pre-fix value"
        );
    }
}

#[test]
fn dark_hover_states_keep_or_raise_text_contrast() {
    for theme in ["iron-dark", "terminal-dark"] {
        let ctx = Ctx::new(theme, AccentColor::Teal);
        let text = ctx.var("--text");
        for (selector, alpha) in HOVER_RULES {
            let base = hover_base(selector, &ctx);
            let now = contrast(text, surface(SCREENS_CSS, selector, base, &ctx));
            // Pre-fix: the slate card-hover triple at the same alpha, as numbers.
            let old_fill = [28.0, 33.0, 41.0, alpha as f64 / 100.0];
            let before = contrast(text, over(old_fill, base));
            println!("measured: {theme} --text on `{selector}` {now:.2}:1 (pre-fix {before:.2}:1)");
            assert!(
                now >= before,
                "{theme}: --text on `{selector}` fell from {before:.2}:1 to {now:.2}:1"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Plan 13 (G-54.4-4) — the third token family (design-tokens.css + warp-ih.css)
// ---------------------------------------------------------------------------

/// Non-test text of `app.rs`.
fn app_rs_render_code() -> &'static str {
    APP_RS
        .split("\n#[cfg(test)]\n")
        .next()
        .expect("app.rs has a non-test region")
}

/// True when some rule of `css` has a selector item exactly equal to one of
/// `items` and declares at least one `--` property.
fn declares_custom_prop_on(css: &str, items: &[String]) -> bool {
    parse_rules(css).iter().any(|r| {
        r.selectors.iter().any(|s| items.contains(s))
            && r.decls.iter().any(|(n, _)| n.starts_with("--"))
    })
}

#[test]
fn html_token_sheets_follow_app_link_order() {
    let code = app_rs_render_code();
    let mut last = 0_usize;
    for needle in [
        "href: TOKENS_CSS",
        "href: DESIGN_TOKENS_CSS",
        "href: WARP_IH_CSS",
        "href: SITE_CSS",
    ] {
        assert_eq!(
            code.matches(needle).count(),
            1,
            "app.rs must link `{needle}` exactly once"
        );
        let at = code.find(needle).expect("counted once above");
        assert!(
            at > last,
            "app.rs links `{needle}` out of order: the html cascade model assumes \
             tokens.css -> design-tokens.css -> warp-ih.css -> site.css"
        );
        last = at;
    }

    // Positive anchor: each token sheet really declares custom properties on :root.
    for (name, css) in HTML_TOKEN_SHEETS {
        assert!(
            declares_custom_prop_on(css, &[":root".to_string()]),
            "{name}: no `:root` rule declares a `--` property (parser or sheet drifted)"
        );
    }

    // Completeness: no sheet linked AFTER site.css declares custom properties on
    // <html>, so the four-sheet model is the whole html-level token cascade.
    let mut html_items: Vec<String> = vec![":root".into(), "html".into()];
    html_items.extend(THEMES.iter().map(|t| format!("[data-theme=\"{t}\"]")));
    for (name, css) in [
        ("wheel.css", WHEEL_CSS),
        ("screens.css", SCREENS_CSS),
        ("components.css", COMPONENTS_CSS),
        ("dashboard.css", DASHBOARD_CSS),
        ("logs.css", LOGS_CSS),
        ("kanban.css", KANBAN_CSS),
        ("voice.css", VOICE_CSS),
        ("bots.css", BOTS_CSS),
        ("tools.css", TOOLS_CSS),
    ] {
        assert!(
            !css.is_empty() && css.contains('{'),
            "{name}: positive anchor failed (empty or unparsable sheet)"
        );
        assert!(
            !declares_custom_prop_on(css, &html_items),
            "{name} declares a `--` property on <html>: the four-sheet html cascade \
             model in theme_contrast.rs is incomplete"
        );
    }
}

#[test]
fn html_cascade_agrees_with_site_theme_tokens() {
    for theme in THEMES {
        let ctx = Ctx::new(theme, AccentColor::Teal);
        let site = theme_tokens(theme);
        for key in REQUIRED_TOKENS {
            let from_html = q(html_value(&ctx, key));
            let from_site = q(site[key]);
            assert_eq!(
                from_html, from_site,
                "{theme}: html cascade resolves `{key}` to {from_html:?}, site.css theme \
                 block says {from_site:?}"
            );
        }
    }
    // The model reproduces the clobber this gap was caused by: design-tokens.css
    // is linked after tokens.css, so on slate-dark `--fg` is ITS value, not the
    // themed oklch one in tokens.css.
    let ctx = Ctx::new("slate-dark", AccentColor::Teal);
    assert_eq!(
        q(html_value(&ctx, "--fg")),
        [201, 209, 217, 100],
        "slate-dark `--fg` must resolve to design-tokens.css's value (link-order clobber)"
    );
}

#[test]
fn bot_roster_card_is_light_and_legible_on_light_themes() {
    let mut min_lum = f64::MAX;
    let mut min_text = f64::MAX;
    let mut min_meta = f64::MAX;
    for theme in LIGHT_THEMES {
        for accent in ACCENTS {
            let ctx = Ctx::new(theme, accent);
            let card = surface(BOTS_CSS, ".kn-bot-card", ctx.var("--bg"), &ctx);
            let lum = luminance(card);
            assert!(
                lum > 0.5,
                "{theme}/{accent:?}: `.kn-bot-card` luminance {lum:.3} is not light"
            );
            let text = contrast(
                over(decl_color(BOTS_CSS, ".kn-bot-card", "color", &ctx), card),
                card,
            );
            let meta = contrast(
                over(
                    decl_color(BOTS_CSS, ".kn-bot-card-meta", "color", &ctx),
                    card,
                ),
                card,
            );
            assert!(
                text >= 4.5,
                "{theme}/{accent:?}: `.kn-bot-card` text {text:.2}:1 is below 4.5:1"
            );
            assert!(
                meta >= 4.5,
                "{theme}/{accent:?}: `.kn-bot-card-meta` {meta:.2}:1 is below 4.5:1"
            );
            min_lum = min_lum.min(lum);
            min_text = min_text.min(text);
            min_meta = min_meta.min(meta);
        }
    }
    println!(
        "measured: .kn-bot-card luminance min {min_lum:.3}, --fg min {min_text:.2}:1, meta min {min_meta:.2}:1"
    );
}

#[test]
fn family3_light_block_matches_light_themes() {
    for theme in THEMES {
        let item = format!("[data-theme=\"{theme}\"]");
        let has_block = parse_rules(SITE_CSS)
            .iter()
            .any(|r| r.selectors.contains(&item) && r.decls.iter().any(|(n, _)| n == "--w-bg-3"));
        let bg_luminance = luminance(theme_tokens(theme)["--bg"]);
        let is_light = bg_luminance > 0.5;
        assert_eq!(
            has_block, is_light,
            "{theme}: --bg luminance {bg_luminance:.3} (light = {is_light}) but site.css \
             light family-3 block present = {has_block}"
        );
    }
}

#[test]
fn orb_state_colours_tolerate_color_mix_tokens() {
    for (name, js) in [("orb.js", ORB_JS), ("avatar.js", AVATAR_JS)] {
        assert!(
            js.contains("function resolveColorToken("),
            "{name}: `resolveColorToken` is not defined"
        );
        for prop in ["--accent-primary", "--danger", "--warn", "--success"] {
            // Assembled at runtime so the needles cannot match this file's source.
            let guarded = format!("resolve{}(canvas, '{prop}'", "ColorToken");
            let raw = format!("resolve{}(canvas, '{prop}'", "Token");
            assert!(
                js.contains(&guarded),
                "{name}: the `{prop}` read must go through the colour guard"
            );
            assert!(
                !js.contains(&raw),
                "{name}: the `{prop}` read still bypasses the colour guard"
            );
        }
        let motion = format!("resolve{}(canvas, '--orb-motion'", "Token");
        assert!(
            js.contains(&motion),
            "{name}: the non-colour `--orb-motion` read must stay on the plain resolver"
        );
    }
}

// ---------------------------------------------------------------------------
// Plan 13 Task 2 — Kanban, pickers and header surfaces follow the light theme
// ---------------------------------------------------------------------------

/// Contrast of an opaque-or-translucent `ink` painted on `surf`.
fn ink_ratio(ink: Rgba, surf: Rgba) -> f64 {
    contrast(over(ink, surf), surf)
}

/// Running minimum per label, printed at the end of a test.
#[derive(Default)]
struct Mins(std::collections::BTreeMap<String, f64>);

impl Mins {
    fn note(&mut self, label: &str, value: f64) {
        let slot = self.0.entry(label.to_string()).or_insert(f64::MAX);
        *slot = slot.min(value);
    }

    fn report(&self, what: &str) {
        for (label, value) in &self.0 {
            println!("measured: {what} {label} min {value:.3}");
        }
    }
}

/// Assert `surf` is light, record its luminance under `label`.
fn assert_light(label: &str, who: &str, surf: Rgba, mins: &mut Mins) {
    let lum = luminance(surf);
    assert!(
        lum > 0.5,
        "{who}: `{label}` luminance {lum:.3} is not light"
    );
    mins.note(&format!("{label} luminance"), lum);
}

/// Assert `selector`'s `color` is at least 4.5:1 on `surf`, record the ratio.
fn assert_ink(who: &str, css: &str, selector: &str, surf: Rgba, ctx: &Ctx, mins: &mut Mins) {
    let ratio = ink_ratio(decl_color(css, selector, "color", ctx), surf);
    assert!(
        ratio >= 4.5,
        "{who}: `{selector}` text {ratio:.2}:1 is below 4.5:1"
    );
    mins.note(&format!("{selector} text"), ratio);
}

#[test]
fn oklch_conversion_matches_reference_points() {
    let ctx = Ctx::new("slate-dark", AccentColor::Teal);
    for (expr, want) in [
        ("oklch(100% 0 0)", [255_i64, 255, 255]),
        ("oklch(0% 0 0)", [0, 0, 0]),
        ("oklch(62.8% 0.2577 29.23)", [255, 0, 0]),
    ] {
        let got = eval(expr, &ctx);
        for (ch, w) in want.iter().enumerate() {
            assert!(
                (got[ch] - *w as f64).abs() <= 1.0,
                "{expr}: channel {ch} is {:.2}, expected {w} within 1",
                got[ch]
            );
        }
    }
}

#[test]
fn kanban_surfaces_are_light_and_legible_on_light_themes() {
    let mut mins = Mins::default();
    for theme in LIGHT_THEMES {
        for accent in ACCENTS {
            let who = format!("{theme}/{accent:?}");
            let ctx = Ctx::new(theme, accent);
            let page = ctx.var("--bg");

            let column = surface(KANBAN_CSS, ".kn-column", page, &ctx);
            assert_light(".kn-column", &who, column, &mut mins);
            assert_ink(
                &who,
                KANBAN_CSS,
                ".kn-column-header",
                column,
                &ctx,
                &mut mins,
            );
            assert_ink(
                &who,
                KANBAN_CSS,
                ".kn-column-count",
                column,
                &ctx,
                &mut mins,
            );

            let card = surface(KANBAN_CSS, ".kn-card", column, &ctx);
            assert_light(".kn-card", &who, card, &mut mins);
            assert_ink(&who, KANBAN_CSS, ".kn-card-heading", card, &ctx, &mut mins);
            assert_ink(&who, KANBAN_CSS, ".kn-card-action", card, &ctx, &mut mins);

            let chip = surface(KANBAN_CSS, ".kn-chip", card, &ctx);
            assert_light(".kn-chip", &who, chip, &mut mins);
            let mut chip_selectors = vec![".kn-chip".to_string()];
            for status in [
                "triage", "todo", "ready", "running", "blocked", "done", "archived",
            ] {
                chip_selectors.push(format!(
                    ".kn-chip[data-kind=\"status\"][data-status=\"{status}\"]"
                ));
            }
            for kind in ["priority", "assignee"] {
                chip_selectors.push(format!(".kn-chip[data-kind=\"{kind}\"]"));
            }
            for selector in &chip_selectors {
                let ratio = ink_ratio(decl_color(KANBAN_CSS, selector, "color", &ctx), chip);
                assert!(
                    ratio >= 4.5,
                    "{who}: chip `{selector}` text {ratio:.2}:1 is below 4.5:1"
                );
                mins.note("chip text (all kinds)", ratio);
            }

            let drawer = surface(KANBAN_CSS, ".kn-drawer", page, &ctx);
            assert_light(".kn-drawer", &who, drawer, &mut mins);
            assert_ink(&who, KANBAN_CSS, ".kn-drawer", drawer, &ctx, &mut mins);
            assert_ink(
                &who,
                KANBAN_CSS,
                ".kn-drawer-title",
                drawer,
                &ctx,
                &mut mins,
            );

            let modal = surface(KANBAN_CSS, ".kn-modal", page, &ctx);
            assert_light(".kn-modal", &who, modal, &mut mins);
            assert_ink(&who, KANBAN_CSS, ".kn-modal-title", modal, &ctx, &mut mins);
            assert_ink(&who, KANBAN_CSS, ".kn-modal-desc", modal, &ctx, &mut mins);

            let menu = surface(KANBAN_CSS, ".kn-profile-menu", page, &ctx);
            assert_light(".kn-profile-menu", &who, menu, &mut mins);
            assert_ink(
                &who,
                KANBAN_CSS,
                ".kn-profile-menu-item-name",
                menu,
                &ctx,
                &mut mins,
            );
            assert_ink(
                &who,
                KANBAN_CSS,
                ".kn-profile-menu-item--all",
                menu,
                &ctx,
                &mut mins,
            );

            let hover = surface(KANBAN_CSS, ".kn-profile-menu-item:hover", menu, &ctx);
            assert_light(".kn-profile-menu-item:hover", &who, hover, &mut mins);
            let ratio = ink_ratio(
                decl_color(KANBAN_CSS, ".kn-profile-menu-item-name", "color", &ctx),
                hover,
            );
            assert!(
                ratio >= 4.5,
                "{who}: menu item name on hover {ratio:.2}:1 is below 4.5:1"
            );
            mins.note("menu item name on hover", ratio);
        }
    }
    mins.report("kanban");
}

#[test]
fn model_pickers_are_light_and_legible_on_light_themes() {
    let mut mins = Mins::default();
    for theme in LIGHT_THEMES {
        for accent in ACCENTS {
            let who = format!("{theme}/{accent:?}");
            let ctx = Ctx::new(theme, accent);
            let page = ctx.var("--bg");
            let select = ".voice-settings-select";
            let rest_fill = eval(
                &bg_colour(&last_decl(VOICE_CSS, select, "background")),
                &ctx,
            );
            let hover_fill = decl_color(
                VOICE_CSS,
                ".voice-settings-select:hover:not(:disabled)",
                "background-color",
                &ctx,
            );
            for (state, fill) in [("rest", rest_fill), ("hover", hover_fill)] {
                let surf = over(fill, page);
                assert_light(&format!("{select} ({state})"), &who, surf, &mut mins);
                let ratio = ink_ratio(decl_color(VOICE_CSS, select, "color", &ctx), surf);
                assert!(
                    ratio >= 4.5,
                    "{who}: `{select}` text on {state} {ratio:.2}:1 is below 4.5:1"
                );
                mins.note(&format!("{select} text ({state})"), ratio);
            }
        }
    }
    mins.report("pickers");
}

#[test]
fn header_components_are_light_and_legible_on_light_themes() {
    let mut mins = Mins::default();
    for theme in LIGHT_THEMES {
        for accent in ACCENTS {
            let who = format!("{theme}/{accent:?}");
            let ctx = Ctx::new(theme, accent);
            let page = ctx.var("--bg");

            let menu = surface(COMPONENTS_CSS, ".ih-menu", page, &ctx);
            assert_light(".ih-menu", &who, menu, &mut mins);
            assert_ink(
                &who,
                COMPONENTS_CSS,
                ".ih-menu .item",
                menu,
                &ctx,
                &mut mins,
            );

            let item_hover = surface(COMPONENTS_CSS, ".ih-menu .item:hover", menu, &ctx);
            assert_light(".ih-menu .item:hover", &who, item_hover, &mut mins);
            assert_ink(
                &who,
                COMPONENTS_CSS,
                ".ih-menu .item:hover",
                item_hover,
                &ctx,
                &mut mins,
            );

            let btn = surface(COMPONENTS_CSS, ".ih-btn", page, &ctx);
            assert_light(".ih-btn", &who, btn, &mut mins);
            assert_ink(&who, COMPONENTS_CSS, ".ih-btn", btn, &ctx, &mut mins);
        }
    }
    mins.report("header");
}

/// Every `--name` referenced as `var(--name)` / `var(--name, ...)` in `css`.
fn referenced_vars(css: &str) -> std::collections::BTreeSet<String> {
    let css = strip_css_comments(css);
    let mut out = std::collections::BTreeSet::new();
    for (at, _) in css.match_indices("var(") {
        let rest = &css[at + 4..];
        let name: String = rest
            .chars()
            .take_while(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '-')
            .collect();
        if name.starts_with("--") && matches!(rest[name.len()..].chars().next(), Some(')' | ',')) {
            out.insert(name);
        }
    }
    out
}

/// True when the RAW html value of `name` is a colour expression, following
/// `var()` indirections. Shadows, lengths and fonts are not colours.
fn is_colour_expression(props: &HashMap<String, String>, name: &str, depth: u32) -> bool {
    assert!(depth < 16, "custom property `{name}` chains too deeply");
    let Some(raw) = props.get(name) else {
        return false;
    };
    let v = raw.trim();
    if v.starts_with('#')
        || v.starts_with("rgb")
        || v.starts_with("color-mix(")
        || v.starts_with("oklch(")
    {
        return true;
    }
    match v.strip_prefix("var(").and_then(|r| r.strip_suffix(')')) {
        Some(inner) if !inner.contains(',') => is_colour_expression(props, inner.trim(), depth + 1),
        _ => false,
    }
}

/// Every colour token declared on `:root` by design-tokens.css or warp-ih.css
/// and consumed by kanban.css, bots.css, voice.css or components.css.
fn consumed_family3_colour_tokens() -> Vec<String> {
    let mut declared = std::collections::BTreeSet::new();
    for css in [DESIGN_TOKENS_CSS, WARP_IH_CSS] {
        for rule in parse_rules(css) {
            if rule.selectors.iter().any(|s| s == ":root") {
                for (name, _) in rule.decls {
                    if name.starts_with("--") {
                        declared.insert(name);
                    }
                }
            }
        }
    }
    let mut consumed = std::collections::BTreeSet::new();
    for css in [KANBAN_CSS, BOTS_CSS, VOICE_CSS, COMPONENTS_CSS] {
        consumed.extend(referenced_vars(css));
    }
    let props = html_custom_props("slate-dark");
    declared
        .intersection(&consumed)
        .filter(|n| is_colour_expression(&props, n, 0))
        .cloned()
        .collect()
}

#[test]
fn every_consumed_family3_colour_token_has_a_light_value() {
    let names = consumed_family3_colour_tokens();
    println!(
        "measured: {} consumed family-3 colour tokens: {names:?}",
        names.len()
    );
    for anchor in [
        "--w-bg-1",
        "--w-bg-2",
        "--w-bg-3",
        "--fg",
        "--accent-primary",
        "--brand",
        "--ansi-yellow",
    ] {
        assert!(
            names.iter().any(|n| n == anchor),
            "positive anchor `{anchor}` is missing from the consumed set: {names:?}"
        );
    }
    assert!(
        names.len() >= 20,
        "expected at least 20 consumed family-3 colour tokens, found {}: {names:?}",
        names.len()
    );
    let dark = Ctx::new("slate-dark", AccentColor::Teal);
    for theme in LIGHT_THEMES {
        let ctx = Ctx::new(theme, AccentColor::Teal);
        for name in &names {
            let (light, base) = (q(ctx.var(name)), q(dark.var(name)));
            assert_ne!(
                light, base,
                "{theme}: `{name}` resolves to its slate-dark value {base:?}: the light \
                 family-3 block in site.css does not cover it"
            );
        }
    }
}

#[test]
fn family3_tokens_unchanged_on_dark_themes() {
    let expected: [(&str, [i64; 3]); 21] = [
        ("--w-bg-0", [10, 14, 19]),
        ("--w-bg-1", [13, 17, 23]),
        ("--w-bg-2", [22, 27, 34]),
        ("--w-bg-3", [28, 35, 44]),
        ("--w-bg-4", [42, 51, 64]),
        ("--w-border", [31, 38, 48]),
        ("--w-border-hi", [48, 54, 61]),
        ("--bg-elevated", [22, 27, 34]),
        ("--border-subtle", [48, 54, 61]),
        ("--fg", [201, 209, 217]),
        ("--fg-dim", [129, 139, 152]),
        ("--fg-strong", [240, 246, 252]),
        ("--fg-disabled", [72, 79, 88]),
        ("--success", [63, 185, 80]),
        ("--warn", [210, 153, 34]),
        ("--danger", [248, 81, 73]),
        ("--accent-primary", [78, 201, 176]),
        ("--accent-primary-hi", [111, 216, 194]),
        ("--ansi-yellow", [210, 153, 34]),
        ("--ansi-bright-cyan", [111, 216, 194]),
        ("--brand", [240, 136, 62]),
    ];
    for theme in DARK_THEMES {
        let ctx = Ctx::new(theme, AccentColor::Teal);
        for (name, [r, g, b]) in expected {
            assert_eq!(
                q(ctx.var(name)),
                [r, g, b, 100],
                "{theme}: `{name}` drifted from its pre-fix dark value"
            );
        }
    }
    // No dark-theme selector of site.css may gain a family-3 declaration.
    let family3: Vec<&str> = expected.iter().map(|(n, _)| *n).collect();
    for rule in parse_rules(SITE_CSS) {
        let dark_item = rule.selectors.iter().any(|s| {
            DARK_THEMES
                .iter()
                .any(|t| *s == format!("[data-theme=\"{t}\"]"))
        });
        if dark_item {
            for (name, _) in &rule.decls {
                assert!(
                    !family3.contains(&name.as_str()),
                    "a dark-theme rule in site.css declares family-3 token `{name}`"
                );
            }
        }
    }
}

/// Every family-3 ink must meet AA against every light text surface, and the
/// one inverted pairing (`--bg` text on an `--accent-primary` fill) must too.
/// This gates the text of every consumer outside the four asserted sheets
/// (screens.css chat chip, the tokens.css / design-tokens.css utility classes,
/// the inline styles in the component files).
///
/// Excluded on purpose: `--w-bg-0` (in the four consumer sheets it paints only
/// the voice-mode orb canvas and state pill, known item (a)) and
/// `--fg-disabled` (inactive text is exempt from WCAG 1.4.3, known item (i)).
#[test]
fn family3_inks_meet_aa_on_light_text_surfaces() {
    let inks = [
        "--fg",
        "--fg-dim",
        "--fg-strong",
        "--accent-primary",
        "--accent-primary-hi",
        "--ansi-bright-cyan",
        "--success",
        "--warn",
        "--ansi-yellow",
        "--danger",
        "--brand",
    ];
    let surfaces = [
        "--bg",
        "--bg-card",
        "--bg-card-hover",
        "--bg-input",
        "--w-bg-1",
        "--w-bg-2",
        "--w-bg-3",
        "--bg-elevated",
    ];
    let mut overall = (f64::MAX, String::new());
    let mut inverted_min = f64::MAX;
    for theme in LIGHT_THEMES {
        for accent in ACCENTS {
            let ctx = Ctx::new(theme, accent);
            let page = ctx.var("--bg");
            for surf_name in surfaces {
                let surf = over(ctx.var(surf_name), page);
                for ink_name in inks {
                    let ratio = ink_ratio(ctx.var(ink_name), surf);
                    assert!(
                        ratio >= 4.5,
                        "{theme}/{accent:?}: ink `{ink_name}` on `{surf_name}` is {ratio:.2}:1, \
                         below 4.5:1"
                    );
                    if ratio < overall.0 {
                        overall = (
                            ratio,
                            format!("{theme}/{accent:?} {ink_name} on {surf_name}"),
                        );
                    }
                }
            }
            let fill = over(ctx.var("--accent-primary"), page);
            let inverted = ink_ratio(ctx.var("--bg"), fill);
            assert!(
                inverted >= 4.5,
                "{theme}/{accent:?}: `--bg` text on an `--accent-primary` fill is \
                 {inverted:.2}:1, below 4.5:1"
            );
            inverted_min = inverted_min.min(inverted);
        }
    }
    println!(
        "measured: family-3 ink min {:.2}:1 ({}), --bg on accent fill min {inverted_min:.2}:1",
        overall.0, overall.1
    );
}

// ---------------------------------------------------------------------------
// Plan 15 (G-54.4-18) — the legacy `--teal` accent ink is legible on light themes
// ---------------------------------------------------------------------------

use crate::components::hermes_app::theme_effects::accent_inline_pair;

/// The Kanban BOARD/TABS toggle's selected segment paints `--teal` text on the
/// `.kn-view-seg` surface. The inline accent used to beat the light themes'
/// own `--teal`, so it read about 1.75:1 on slate-light.
#[test]
fn kanban_view_toggle_active_segment_meets_aa_on_light_themes() {
    let mut min = (f64::MAX, String::new());
    for theme in LIGHT_THEMES {
        for accent in ACCENTS {
            let ctx = Ctx::new(theme, accent);
            let surf = surface(KANBAN_CSS, ".kn-view-seg", ctx.var("--bg"), &ctx);
            let ink = decl_color(
                KANBAN_CSS,
                ".kn-view-seg[aria-checked=\"true\"]",
                "color",
                &ctx,
            );
            let ratio = ink_ratio(ink, surf);
            assert!(
                ratio >= 4.5,
                "{theme}/{accent:?}: `.kn-view-seg[aria-checked=\"true\"]` text {ratio:.2}:1 \
                 is below 4.5:1"
            );
            if ratio < min.0 {
                min = (ratio, format!("{theme}/{accent:?}"));
            }
        }
    }
    println!(
        "measured: .kn-view-seg active segment min {:.2}:1 ({})",
        min.0, min.1
    );
}

/// Dark themes keep today's ink exactly; unknown or tampered theme names fall
/// back to it too (T-54.4-46). Characterization test: passes on the unfixed code.
#[test]
fn accent_inline_pair_is_unchanged_on_dark_themes() {
    for theme in DARK_THEMES {
        for accent in ACCENTS {
            assert_eq!(
                accent_inline_pair(theme, accent),
                accent.hex_pair(),
                "{theme}/{accent:?}: a dark theme must keep `hex_pair()`"
            );
        }
    }
    for theme in ["", "no-such-theme"] {
        for accent in ACCENTS {
            assert_eq!(
                accent_inline_pair(theme, accent),
                accent.hex_pair(),
                "unknown theme {theme:?}/{accent:?} must keep `hex_pair()`"
            );
        }
    }
}

/// The light inks are opaque `#rrggbb`, strictly darker than the dark inks,
/// and the five base inks stay distinct (the picker must keep working).
#[test]
fn light_accent_inks_are_distinct_and_darker_than_dark_inks() {
    let mut bases = Vec::new();
    for accent in ACCENTS {
        let (dark_base, dark_hover) = accent.hex_pair();
        let (light_base, light_hover) = accent.light_hex_pair();
        let parse = |what: &str, hex: &str| {
            let c = parse_hex(hex)
                .unwrap_or_else(|| panic!("{accent:?}: light {what} `{hex}` is not #rrggbb"));
            assert!(
                (c[3] - 1.0).abs() < 1e-9,
                "{accent:?}: light {what} `{hex}` is not opaque"
            );
            c
        };
        let lb = parse("base", light_base);
        let lh = parse("hover", light_hover);
        let db = parse_hex(dark_base).expect("dark base hex");
        let dh = parse_hex(dark_hover).expect("dark hover hex");
        assert!(
            luminance(lb) < luminance(db),
            "{accent:?}: light base {light_base} is not darker than dark base {dark_base}"
        );
        assert!(
            luminance(lh) < luminance(dh),
            "{accent:?}: light hover {light_hover} is not darker than dark hover {dark_hover}"
        );
        bases.push(light_base);
    }
    let mut unique = bases.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(
        unique.len(),
        bases.len(),
        "light base inks are not pairwise distinct: {bases:?}"
    );
}

// ---------------------------------------------------------------------------
// Plan 15 Task 2 — every legacy accent-ink consumer reads AA on light themes
// ---------------------------------------------------------------------------

const THEME_EFFECTS_RS: &str = include_str!("theme_effects.rs");

/// Both inline inks clear 4.5:1 on every light text surface, each surface
/// composited over the page `--bg`.
#[test]
fn legacy_accent_inks_meet_aa_on_light_text_surfaces() {
    let inks = ["--teal", "--teal-bright"];
    let surfaces = [
        "--bg",
        "--bg-deep",
        "--bg-card",
        "--bg-card-hover",
        "--bg-input",
        "--w-bg-1",
        "--w-bg-2",
        "--w-bg-3",
        "--bg-elevated",
    ];
    let mut overall = (f64::MAX, String::new());
    for theme in LIGHT_THEMES {
        for accent in ACCENTS {
            let ctx = Ctx::new(theme, accent);
            let page = ctx.var("--bg");
            for surf_name in surfaces {
                let surf = over(ctx.var(surf_name), page);
                for ink_name in inks {
                    let ratio = ink_ratio(ctx.var(ink_name), surf);
                    assert!(
                        ratio >= 4.5,
                        "{theme}/{accent:?}: ink `{ink_name}` on `{surf_name}` is {ratio:.2}:1, \
                         below 4.5:1"
                    );
                    if ratio < overall.0 {
                        overall = (
                            ratio,
                            format!("{theme}/{accent:?} {ink_name} on {surf_name}"),
                        );
                    }
                }
            }
        }
    }
    println!(
        "measured: legacy accent ink min {:.2}:1 ({})",
        overall.0, overall.1
    );
}

/// The consumers the debug session measured at 1.55-1.9:1 now read AA.
#[test]
fn legacy_accent_consumers_meet_aa_on_light_themes() {
    let mut chat_edit = Mins::default();
    let mut new_profile = Mins::default();
    let mut screen_tag = Mins::default();
    let mut module_value = Mins::default();
    let mut nav_active = Mins::default();
    let mut tab_active = Mins::default();
    for theme in LIGHT_THEMES {
        for accent in ACCENTS {
            let ctx = Ctx::new(theme, accent);
            let who = format!("{theme}/{accent:?}");
            let page = ctx.var("--bg");

            // (1) roster CHAT/EDIT: `.btn` on a `.kn-bot-card`.
            let card = surface(BOTS_CSS, ".kn-bot-card", page, &ctx);
            let btn_on_card = surface(SITE_CSS, ".btn", card, &ctx);
            assert_ink(&who, SITE_CSS, ".btn", btn_on_card, &ctx, &mut chat_edit);

            // (2) '+ NEW PROFILE': `.btn` on the page.
            let btn_on_page = surface(SITE_CSS, ".btn", page, &ctx);
            assert_ink(&who, SITE_CSS, ".btn", btn_on_page, &ctx, &mut new_profile);

            // (3) `.screen-tag` on `--bg` and `--bg-deep`.
            for surf_name in ["--bg", "--bg-deep"] {
                let surf = over(ctx.var(surf_name), page);
                assert_ink(
                    &who,
                    SCREENS_CSS,
                    ".screen-tag",
                    surf,
                    &ctx,
                    &mut screen_tag,
                );
            }

            // (4) status strip MODULE value.
            let header = surface(DASHBOARD_CSS, ".ih-header", page, &ctx);
            let strip = surface(DASHBOARD_CSS, ".ih-status-strip", header, &ctx);
            assert_ink(
                &who,
                SITE_CSS,
                ".ih-status-strip .ih-seg-module .v",
                strip,
                &ctx,
                &mut module_value,
            );

            // (5) active header shortcut.
            let nav = surface(DASHBOARD_CSS, ".ih-nav-btn", header, &ctx);
            assert_ink(
                &who,
                DASHBOARD_CSS,
                ".ih-nav-btn[aria-current=\"page\"]",
                nav,
                &ctx,
                &mut nav_active,
            );

            // (6) `.tab.is-active` on `--bg` and `--bg-card`.
            for surf_name in ["--bg", "--bg-card"] {
                let surf = over(ctx.var(surf_name), page);
                assert_ink(
                    &who,
                    SCREENS_CSS,
                    ".tab.is-active",
                    surf,
                    &ctx,
                    &mut tab_active,
                );
            }
        }
    }
    chat_edit.report("roster CHAT/EDIT");
    new_profile.report("+ NEW PROFILE");
    screen_tag.report("screen tag");
    module_value.report("status MODULE value");
    nav_active.report("active header shortcut");
    tab_active.report("active tab");
}

/// Accent fills keep their light text legible on both light themes.
#[test]
fn accent_fills_keep_light_text_legible_on_light_themes() {
    let mut hover_min = (f64::MAX, String::new());
    let mut primary_min = (f64::MAX, String::new());
    for theme in LIGHT_THEMES {
        for accent in ACCENTS {
            let ctx = Ctx::new(theme, accent);
            let who = format!("{theme}/{accent:?}");

            let hover_fill = surface(SITE_CSS, ".btn:hover", ctx.var("--bg"), &ctx);
            let hover = ink_ratio(
                decl_color(SITE_CSS, ".btn:hover", "color", &ctx),
                hover_fill,
            );
            assert!(
                hover >= 4.5,
                "{who}: `.btn:hover` text on its --teal fill is {hover:.2}:1, below 4.5:1"
            );
            if hover < hover_min.0 {
                hover_min = (hover, who.clone());
            }

            let primary_fill =
                surface(DASHBOARD_CSS, ".ih-btn-primary", ctx.var("--bg-card"), &ctx);
            let primary = ink_ratio(
                decl_color(DASHBOARD_CSS, ".ih-btn-primary", "color", &ctx),
                primary_fill,
            );
            assert!(
                primary >= 4.5,
                "{who}: `.ih-btn-primary` text on its --teal fill is {primary:.2}:1, below 4.5:1"
            );
            if primary < primary_min.0 {
                primary_min = (primary, who);
            }
        }
    }
    println!(
        "measured: .btn:hover fill text min {:.2}:1 ({}), .ih-btn-primary fill text min {:.2}:1 ({})",
        hover_min.0, hover_min.1, primary_min.0, primary_min.1
    );
}

/// `is_light_theme` agrees with each theme's real `--bg` luminance, so a new
/// light theme cannot ship with the bright dark-theme ink.
#[test]
fn light_theme_classification_matches_theme_luminance() {
    use crate::components::hermes_app::theme_effects::is_light_theme;
    for theme in THEMES {
        let bg = theme_tokens(theme)["--bg"];
        let lum = luminance(bg);
        assert_eq!(
            is_light_theme(theme),
            lum > 0.5,
            "{theme}: is_light_theme disagrees with --bg luminance {lum:.3}"
        );
        assert_eq!(
            is_light_theme(theme),
            LIGHT_THEMES.contains(&theme),
            "{theme}: is_light_theme disagrees with LIGHT_THEMES"
        );
    }
}

/// Dark themes render byte-for-byte as before: `.screen-tag` and `.btn` resolve
/// to exactly `hex_pair().0`. Characterization test: passes on the unfixed code.
#[test]
fn legacy_accent_consumers_unchanged_on_dark_themes() {
    for theme in DARK_THEMES {
        for accent in ACCENTS {
            let ctx = Ctx::new(theme, accent);
            let want = q(parse_hex(accent.hex_pair().0).expect("dark base hex"));
            assert_eq!(
                q(decl_color(SCREENS_CSS, ".screen-tag", "color", &ctx)),
                want,
                "{theme}/{accent:?}: `.screen-tag` is not the dark accent hex"
            );
            assert_eq!(
                q(decl_color(SITE_CSS, ".btn", "color", &ctx)),
                want,
                "{theme}/{accent:?}: `.btn` is not the dark accent hex"
            );
        }
    }
}

/// Effect 3 is pinned to the shared helper and to theme changes, by counting
/// the non-comment source of `theme_effects.rs` (it has no test module).
#[test]
fn theme_effects_writes_the_theme_aware_accent_pair() {
    let code: String = THEME_EFFECTS_RS
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    for (needle, want) in [
        ("accent_inline_pair(", 2),
        (".hex_pair()", 1),
        (".light_hex_pair()", 1),
        ("set_property(\"--teal\",", 1),
        ("set_property(\"--teal-bright\",", 1),
        ("theme.read()", 2),
    ] {
        let got = code.matches(needle).count();
        assert_eq!(
            got, want,
            "theme_effects.rs (comments stripped) has {got} occurrence(s) of `{needle}`, \
             expected {want}"
        );
    }
}
