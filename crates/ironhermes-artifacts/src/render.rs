//! Markdown/HTML rendering for artifact bodies.
//!
//! Neither source format is sanitized here — isolation is the sandbox/CSP
//! layer's job (serving route + iframe sandbox, Plans 03/05), not the
//! renderer's (RESEARCH Pitfall 3). This module's only jobs are markdown
//! conversion and size measurement.

use pulldown_cmark::{Options, Parser, html};
use serde::{Deserialize, Serialize};

/// The source format of an artifact body.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SourceFormat {
    Html,
    Markdown,
    /// A code/script deliverable (python/rust/c/bash/etc). Rendered as an
    /// escaped plain-monospace `<pre><code>` block — no syntax highlighting,
    /// no language tag persisted (D-01/D-02, `52.1-CONTEXT.md`). The enum
    /// deliberately carries no language field: highlighting is deferred, so
    /// there is nothing for a language tag to drive yet.
    Code,
}

impl SourceFormat {
    /// Parse a source format from its canonical wire string ("html" | "md" | "code").
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "html" => Some(SourceFormat::Html),
            "md" => Some(SourceFormat::Markdown),
            "code" => Some(SourceFormat::Code),
            _ => None,
        }
    }

    /// The canonical wire string for this format ("html" | "md" | "code").
    pub fn as_str(&self) -> &'static str {
        match self {
            SourceFormat::Html => "html",
            SourceFormat::Markdown => "md",
            SourceFormat::Code => "code",
        }
    }

    /// The default filename extension for a raw-source download of this
    /// format (D-03, consumed by Plan 02). `txt` for `Code` because the enum
    /// deliberately carries no language tag.
    pub fn default_extension(&self) -> &'static str {
        match self {
            SourceFormat::Html => "html",
            SourceFormat::Markdown => "md",
            SourceFormat::Code => "txt",
        }
    }
}

/// Escape the three HTML-significant characters in a text node: `&`, `<`,
/// `>`. The ampersand replacement runs FIRST — reversing the order would
/// double-unescape (an already-escaped `&lt;` would become `&amp;lt;` only
/// under this ordering; the reverse order corrupts it). Only these three
/// characters are escaped: the body is always a text node, never an
/// attribute value, so quote characters need no escaping (RESEARCH Pitfall 5,
/// the Don't-Hand-Roll table's single narrowly-scoped exception).
///
/// `pub` so callers that interpolate untrusted, attacker-influenceable text
/// into a Markdown document (e.g. `ironhermes-tools`' pointer-artifact body,
/// CR-01) can neutralize it with the exact same ordering this module already
/// uses for the `Code` render arm, instead of hand-rolling a second escaper.
pub fn escape_html_text(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Render `body` per `format`. HTML passes through unchanged; Markdown is
/// converted via pulldown-cmark with tables + strikethrough enabled; Code is
/// HTML-escaped and wrapped in a plain-monospace `<pre><code>` block.
pub fn render(format: SourceFormat, body: &str) -> String {
    match format {
        SourceFormat::Html => body.to_string(),
        SourceFormat::Markdown => {
            let options = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH;
            let parser = Parser::new_ext(body, options);
            let mut html_out = String::new();
            html::push_html(&mut html_out, parser);
            html_out
        }
        SourceFormat::Code => {
            format!("<pre><code>{}</code></pre>", escape_html_text(body))
        }
    }
}

/// The byte length of the rendered output for `format`/`body`.
pub fn rendered_len(format: SourceFormat, body: &str) -> usize {
    render(format, body).len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_passes_through_unchanged() {
        assert_eq!(render(SourceFormat::Html, "<b>x</b>"), "<b>x</b>");
    }

    #[test]
    fn markdown_renders_heading() {
        let out = render(SourceFormat::Markdown, "# Title");
        assert!(out.contains("<h1>"));
    }

    #[test]
    fn markdown_renders_table() {
        let md = "| a | b |\n|---|---|\n| 1 | 2 |\n";
        let out = render(SourceFormat::Markdown, md);
        assert!(out.contains("<table>"));
    }

    #[test]
    fn rendered_len_matches_render_output() {
        let out = render(SourceFormat::Markdown, "# Title");
        assert_eq!(rendered_len(SourceFormat::Markdown, "# Title"), out.len());
        let html_body = "<p>hi</p>";
        assert_eq!(rendered_len(SourceFormat::Html, html_body), html_body.len());
    }

    #[test]
    fn source_format_parses_from_strings() {
        assert_eq!(SourceFormat::parse("html"), Some(SourceFormat::Html));
        assert_eq!(SourceFormat::parse("md"), Some(SourceFormat::Markdown));
        assert_eq!(SourceFormat::parse("code"), Some(SourceFormat::Code));
        assert_eq!(SourceFormat::parse("bogus"), None);
    }

    #[test]
    fn source_format_code_round_trips_wire_string() {
        assert_eq!(SourceFormat::Code.as_str(), "code");
        assert_eq!(
            SourceFormat::parse(SourceFormat::Code.as_str()),
            Some(SourceFormat::Code)
        );
    }

    #[test]
    fn default_extension_matches_each_variant() {
        assert_eq!(SourceFormat::Html.default_extension(), "html");
        assert_eq!(SourceFormat::Markdown.default_extension(), "md");
        assert_eq!(SourceFormat::Code.default_extension(), "txt");
    }

    /// Ampersand-first ordering (load-bearing): an already-entity-looking
    /// input is escaped a SECOND time rather than left alone, proving `&` is
    /// replaced before `<`/`>` rather than after (the reverse order would
    /// double-unescape instead).
    #[test]
    fn escape_html_text_escapes_ampersand_before_angle_brackets() {
        let out = super::escape_html_text("&lt;script&gt;");
        assert_eq!(out, "&amp;lt;script&amp;gt;");
    }

    #[test]
    fn render_code_wraps_in_pre_code() {
        let out = render(SourceFormat::Code, "print('hi')");
        assert!(out.starts_with("<pre><code>"));
        assert!(out.ends_with("</code></pre>"));
    }

    /// A body containing a closing-code-tag sequence must not let it survive
    /// as a live HTML tag — the rendered output carries exactly one literal
    /// `</code>`, the one the renderer itself emitted.
    #[test]
    fn render_code_body_with_closing_tag_sequence_has_exactly_one_literal_closing_tag() {
        let out = render(SourceFormat::Code, "</code>");
        assert_eq!(out.matches("</code>").count(), 1);
        assert!(out.contains("&lt;/code&gt;"));
    }

    /// T-52.1-01: an inline script element inside the code body must never
    /// appear as an unescaped left angle bracket in the rendered output.
    #[test]
    fn render_code_body_with_inline_script_has_no_unescaped_left_angle_bracket() {
        let body = "<script>alert(1)</script>";
        let out = render(SourceFormat::Code, body);
        let inner = out
            .strip_prefix("<pre><code>")
            .and_then(|s| s.strip_suffix("</code></pre>"))
            .expect("render output must be wrapped in pre/code");
        assert!(
            !inner.contains('<'),
            "escaped body must contain no literal '<': {inner}"
        );
        assert!(inner.contains("&lt;script&gt;"));
    }

    #[test]
    fn rendered_len_matches_render_for_code() {
        let body = "fn main() {}";
        assert_eq!(
            rendered_len(SourceFormat::Code, body),
            render(SourceFormat::Code, body).len()
        );
    }
}
