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
pub fn escape_markdown_v2(_text: &str) -> String {
    unimplemented!()
}

/// Smart escape that preserves real MarkdownV2 syntax.
///
/// Backslash-escapes the 18 reserved chars OUTSIDE of fenced code blocks
/// (` ``` `…` ``` `), inline-code spans (`` ` ``…`` ` ``), and `[label](url)`
/// link URLs. Passes reserved chars through verbatim INSIDE those contexts.
///
/// A `\<reserved>` sequence in the input is preserved as `\<reserved>` and
/// not double-escaped (Pitfall 5 — `\`` does not toggle inline-code state).
pub fn escape_outside_code_blocks(_text: &str) -> String {
    unimplemented!()
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
        // bare em-dash and final `.` escaped.
        let input = "Hello *world*. Visit [docs](https://x.com/a.b) or run `cat file.txt` — done.";
        let out = escape_outside_code_blocks(input);
        let expected = "Hello *world*\\. Visit [docs](https://x.com/a.b) or run `cat file.txt` \\— done\\.";
        assert_eq!(out, expected);
    }
}
