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
}

impl SourceFormat {
    /// Parse a source format from its canonical wire string ("html" | "md").
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "html" => Some(SourceFormat::Html),
            "md" => Some(SourceFormat::Markdown),
            _ => None,
        }
    }

    /// The canonical wire string for this format ("html" | "md").
    pub fn as_str(&self) -> &'static str {
        match self {
            SourceFormat::Html => "html",
            SourceFormat::Markdown => "md",
        }
    }
}

/// Render `body` per `format`. HTML passes through unchanged; Markdown is
/// converted via pulldown-cmark with tables + strikethrough enabled.
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
        assert_eq!(SourceFormat::parse("bogus"), None);
    }
}
