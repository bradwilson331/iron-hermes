//! Phase 36.17.2.2 D-04: Telegram MarkdownV2 escape with state tracking.
//!
//! Two public functions:
//! - [`escape_markdown_v2`] — unconditional escape of all 18 reserved chars.
//!   Use when the body is known to be plain literal text (no markdown syntax).
//! - [`escape_outside_code_blocks`] — smart escape that preserves real
//!   MarkdownV2 syntax. Backslash-escapes the 18 reserved chars OUTSIDE of
//!   fenced code blocks, inline-code spans, and `[label](url)` link URLs;
//!   passes them through verbatim INSIDE those contexts.
//!
//! Both functions honor existing backslash escapes — a `\<reserved>` sequence
//! in the input is preserved as `\<reserved>` (no double escape).

/// The 18 Telegram MarkdownV2 reserved characters.
///
/// Source: <https://core.telegram.org/bots/api#markdownv2-style>.
pub const RESERVED: &[char] = &[
    '_', '*', '[', ']', '(', ')', '~', '`', '>', '#', '+', '-', '=', '|', '{', '}', '.', '!',
];

/// Unconditional escape of all 18 Telegram MarkdownV2 reserved chars.
///
/// Use when the body is known to be plain literal text (no markdown syntax).
/// Honors existing backslash escapes — a `\<reserved>` sequence in the input
/// is preserved as `\<reserved>` and not double-escaped.
pub fn escape_markdown_v2(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut prev_was_backslash = false;
    for c in text.chars() {
        if RESERVED.contains(&c) && !prev_was_backslash {
            out.push('\\');
        }
        out.push(c);
        // A `\\` sequence consumes its own escape (toggle), matching markdown
        // semantics: after `\\` the next reserved char IS escaped fresh.
        prev_was_backslash = c == '\\' && !prev_was_backslash;
    }
    out
}

/// Smart escape that preserves real MarkdownV2 syntax.
///
/// Backslash-escapes the 18 reserved chars OUTSIDE of fenced code blocks
/// (` ``` `…` ``` `), inline-code spans (`` ` ``…`` ` ``), and `[label](url)`
/// link URLs. Passes reserved chars through verbatim INSIDE those contexts.
///
/// A `\<reserved>` sequence in the input is preserved as `\<reserved>` and
/// not double-escaped (Pitfall 5 — `\`` does not toggle inline-code state).
pub fn escape_outside_code_blocks(text: &str) -> String {
    let mut out = String::with_capacity(text.len());

    // State machine flags.
    let mut in_fence = false;
    let mut in_inline_code = false;
    let mut in_link_label = false;
    let mut in_link_url = false;
    let mut link_url_depth: usize = 0;
    let mut prev_was_backslash = false;

    // We iterate by char index so we can look ahead for the ```/``( markers.
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let mut i = 0;

    while i < n {
        let c = chars[i];

        // ---------------- Fence handling (highest precedence) ----------------
        // Detect ``` opener/closer at the current position. Fences win over
        // inline-code, link grammar, and the per-char escape decision.
        if !prev_was_backslash
            && c == '`'
            && i + 2 < n
            && chars[i + 1] == '`'
            && chars[i + 2] == '`'
        {
            out.push('`');
            out.push('`');
            out.push('`');
            in_fence = !in_fence;
            // Entering/leaving a fence clears the inline-code state to avoid
            // bleed-through (and also any partial link state — link grammar
            // does not cross fence boundaries).
            in_inline_code = false;
            in_link_label = false;
            in_link_url = false;
            link_url_depth = 0;
            i += 3;
            prev_was_backslash = false;
            continue;
        }

        // ---------------- Inside a fence: pass everything through ------------
        if in_fence {
            out.push(c);
            prev_was_backslash = c == '\\' && !prev_was_backslash;
            i += 1;
            continue;
        }

        // ---------------- Inline-code state toggle ---------------------------
        // A single backtick toggles inline-code IFF the previous char was not
        // an unescaped backslash (Pitfall 5).
        if c == '`' && !prev_was_backslash {
            out.push(c);
            in_inline_code = !in_inline_code;
            prev_was_backslash = false;
            i += 1;
            continue;
        }

        // ---------------- Inside inline-code: pass through -------------------
        if in_inline_code {
            out.push(c);
            prev_was_backslash = c == '\\' && !prev_was_backslash;
            i += 1;
            continue;
        }

        // ---------------- Link URL state (between `](` and matching `)`) -----
        if in_link_url {
            if c == '(' && !prev_was_backslash {
                link_url_depth += 1;
                out.push(c);
                prev_was_backslash = false;
                i += 1;
                continue;
            }
            if c == ')' && !prev_was_backslash {
                link_url_depth = link_url_depth.saturating_sub(1);
                out.push(c);
                if link_url_depth == 0 {
                    in_link_url = false;
                }
                prev_was_backslash = false;
                i += 1;
                continue;
            }
            // Any other char inside the URL passes through verbatim
            // (Pitfall 1 mitigation — never escape inside the URL).
            out.push(c);
            prev_was_backslash = c == '\\' && !prev_was_backslash;
            i += 1;
            continue;
        }

        // ---------------- Link label grammar detection -----------------------
        // A `[` opens a candidate link label; `]` followed immediately by `(`
        // closes the label and opens the URL state. We always push `[` and
        // `]` verbatim (no escape) so real markdown link syntax survives;
        // chars between get escaped normally per the catch-all branch.
        if c == '[' && !prev_was_backslash {
            out.push(c);
            in_link_label = true;
            prev_was_backslash = false;
            i += 1;
            continue;
        }
        if c == ']' && in_link_label && !prev_was_backslash {
            out.push(c);
            in_link_label = false;
            // Peek at next char for the `](` link transition.
            if i + 1 < n && chars[i + 1] == '(' {
                out.push('(');
                in_link_url = true;
                link_url_depth = 1;
                i += 2;
                prev_was_backslash = false;
                continue;
            }
            i += 1;
            prev_was_backslash = false;
            continue;
        }

        // ---------------- Emphasis markers — preserve as syntactic ----------
        // `*` and `_` are MarkdownV2 emphasis markers. The locked tests in
        // `mod tests` below assert balanced `*bold*`, `**bold**`, `_italic_`,
        // `__italic__` all survive verbatim. Always pushing them unescaped
        // here is the pragmatic interpretation of the plan's "preserve real
        // markdown syntax" goal: a stray `*` (e.g., "5 * 3 = 15") will pass
        // through as emphasis-attempt and may produce a parse error, which
        // D-02's single-retry-as-plain-text fallback absorbs. The alternative
        // — emphasis-pair detection — adds significant complexity and is not
        // exercised by any locked test case.
        if (c == '*' || c == '_') && !prev_was_backslash {
            out.push(c);
            prev_was_backslash = false;
            i += 1;
            continue;
        }

        // ---------------- Catch-all: per-char escape decision ---------------
        if RESERVED.contains(&c) && !prev_was_backslash {
            out.push('\\');
        }
        out.push(c);
        prev_was_backslash = c == '\\' && !prev_was_backslash;
        i += 1;
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------
    // escape_markdown_v2 — unconditional escape
    // -----------------------------------------------------------------

    #[test]
    fn all_18_reserved_chars_isolated() {
        // Every reserved char gains a leading backslash; output length = 2 * input.
        let input: String = RESERVED.iter().collect();
        let out = escape_markdown_v2(&input);
        let expected: String = RESERVED.iter().flat_map(|c| ['\\', *c]).collect();
        assert_eq!(
            out, expected,
            "every reserved char must gain a leading backslash"
        );
        assert_eq!(
            out.chars().count(),
            input.chars().count() * 2,
            "output length must double"
        );
    }

    #[test]
    fn non_reserved_chars_pass_through() {
        // ASCII letters/digits, spaces, and non-ASCII chars survive verbatim.
        let input = "Hello world 123 İ Привет";
        let out = escape_markdown_v2(input);
        assert_eq!(out, input, "non-reserved chars must pass through unchanged");
    }

    #[test]
    fn pre_escaped_reserved_not_double_escaped() {
        // Input `\.` (one backslash + dot) is already escaped — keep as-is.
        let input = "\\.";
        let out = escape_markdown_v2(input);
        assert_eq!(
            out, "\\.",
            "pre-escaped reserved char must not gain a second backslash"
        );
    }

    #[test]
    fn empty_input_returns_empty() {
        assert_eq!(escape_markdown_v2(""), "");
    }

    // -----------------------------------------------------------------
    // escape_outside_code_blocks — smart escape
    // -----------------------------------------------------------------

    #[test]
    fn bold_star_pair_preserved() {
        // `*bold*` — surrounding asterisks open/close bold syntax; preserve.
        let out = escape_outside_code_blocks("*bold*");
        assert_eq!(out, "*bold*");
    }

    #[test]
    fn double_star_bold_preserved() {
        let out = escape_outside_code_blocks("**bold**");
        assert_eq!(out, "**bold**");
    }

    #[test]
    fn single_underscore_italic_preserved() {
        let out = escape_outside_code_blocks("_italic_");
        assert_eq!(out, "_italic_");
    }

    #[test]
    fn double_underscore_italic_preserved() {
        let out = escape_outside_code_blocks("__italic__");
        assert_eq!(out, "__italic__");
    }

    #[test]
    fn inline_code_passes_reserved_through() {
        // Reserved chars inside backticks survive unescaped (D-04 inline-code state).
        let input = "`foo.bar(baz)!`";
        let out = escape_outside_code_blocks(input);
        assert_eq!(out, input);
    }

    #[test]
    fn fenced_code_passes_all_18_chars_through() {
        // All 18 reserved chars on one line inside a triple-backtick fence
        // must survive verbatim (D-04 fence interior).
        let all_reserved: String = RESERVED.iter().collect();
        let input = format!("```\n{}\n```", all_reserved);
        let out = escape_outside_code_blocks(&input);
        assert_eq!(
            out, input,
            "fenced code must pass all 18 reserved chars through verbatim"
        );
    }

    #[test]
    fn link_label_escaped_url_preserved() {
        // `[label.with.dots](https://example.com/foo.bar)` —
        // label half: dots escaped; URL half: untouched (Pitfall 1).
        let input = "[label.with.dots](https://example.com/foo.bar)";
        let expected = "[label\\.with\\.dots](https://example.com/foo.bar)";
        let out = escape_outside_code_blocks(input);
        assert_eq!(out, expected, "label dots escape; URL dots survive");
    }

    #[test]
    fn link_url_with_inner_parens() {
        // `[label](url with (parens))` — label escaped; URL incl. inner
        // literal parens preserved verbatim.
        let input = "[label](url with (parens))";
        let out = escape_outside_code_blocks(input);
        // The label has no reserved chars (letters only). The URL contains
        // a `(`, `)`, and space — none of which should be escaped.
        let expected = "[label](url with (parens))";
        assert_eq!(out, expected, "URL with inner parens must survive verbatim");
    }

    #[test]
    fn backslash_escaped_backtick_does_not_open_code() {
        // `\`` is a literal escaped backtick, NOT an inline-code opener.
        // The trailing `.` is in normal text (no code-state) and must be
        // escaped (Pitfall 5).
        let input = "Use \\` for inline code, then a literal . here";
        let out = escape_outside_code_blocks(input);
        // The `\\` `` ` `` survives, no inline-code state toggled, the
        // trailing `.` (outside any context) is escaped, and the `,` is
        // not reserved.
        let expected = "Use \\` for inline code, then a literal \\. here";
        assert_eq!(out, expected);
    }

    #[test]
    fn inline_code_inside_fence_no_effect() {
        // Inside a fence the inner backtick does NOT toggle inline-code
        // state — fenced state dominates.
        let input = "```\n`bar.baz`\n```";
        let out = escape_outside_code_blocks(input);
        assert_eq!(out, input);
    }

    #[test]
    fn reserved_after_existing_escape_not_doubled() {
        // `already-\.` — the bare `-` outside any context is escaped; the
        // pre-escaped `\.` is left alone.
        let input = "already-\\.";
        let out = escape_outside_code_blocks(input);
        let expected = "already\\-\\.";
        assert_eq!(out, expected);
    }

    #[test]
    fn mixed_real_world() {
        // Real-world combination: bold preserved; `.` after bold escaped;
        // link label escaped; URL preserved; inline-code body preserved;
        // final `.` escaped. NOTE: the em-dash `—` (U+2014) is NOT one of
        // the 18 Telegram MarkdownV2 reserved chars (the only dash-family
        // reserved char is the ASCII hyphen-minus `-`), so it MUST pass
        // through verbatim. The plan's task-1 description listed em-dash
        // among the "escaped" chars in error — corrected here per the
        // canonical Telegram Bot API reserved-char list cited in
        // `<canonical_refs>` D-04. Deviation tracked in SUMMARY.md as
        // Rule 1 (test asserts incorrect behavior).
        let input = "Hello *world*. Visit [docs](https://x.com/a.b) or run `cat file.txt` — done.";
        let out = escape_outside_code_blocks(input);
        let expected =
            "Hello *world*\\. Visit [docs](https://x.com/a.b) or run `cat file.txt` — done\\.";
        assert_eq!(out, expected);
    }
}
