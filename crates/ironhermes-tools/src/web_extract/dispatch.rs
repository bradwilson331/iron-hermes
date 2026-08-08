//! Phase 25.2 D-03: URL classification + backend selection.
//!
//! All functions are pure (no network, no env I/O). Classification runs BEFORE
//! any HTTP request per D-03.
//!
//! Phase 41.3 D-17: the env-order `select_backend()` selector that used to live
//! here was dead code (zero callers — the live selection ladder was always the
//! inline `if std::env::var(...)` chain hand-written inside
//! `web_extract.rs::fetch_web_with_chain`, not this helper). It has been
//! removed; `fetch_web_with_chain` now walks the config-ordered
//! `tools.web_extract.chain` directly. The `Backend` enum below is kept as a
//! plain name/enum pairing (used by its own tests) but is no longer part of
//! any dispatch path.

/// Classification of a URL into one of three dispatch branches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UrlClass {
    /// host matches youtube.com / youtu.be / m.youtube.com / music.youtube.com
    YouTube,
    /// path suffix `.pdf` (case-insensitive, ignoring query string) OR mid-fetch Content-Type
    Pdf,
    /// everything else — goes through the backend chain (Firecrawl → Exa → Tavily → Local)
    Web,
}

/// Named backend for the default web branch. Selection is no longer made by
/// this type (see the module doc-comment) — kept as a plain name/enum pairing
/// exercised by `backend_name_strings` below.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    Firecrawl,
    Exa,
    Tavily,
    Local,
}

impl Backend {
    pub fn name(self) -> &'static str {
        match self {
            Backend::Firecrawl => "firecrawl",
            Backend::Exa => "exa",
            Backend::Tavily => "tavily",
            Backend::Local => "local",
        }
    }
}

/// D-03 classifier. Pure-string examination of the parsed URL.
/// On parse failure, returns `UrlClass::Web` so the default-web branch can surface
/// the underlying network error to the operator (per D-02 partial-success).
pub fn classify_url(url: &str) -> UrlClass {
    let parsed = match url::Url::parse(url) {
        Ok(u) => u,
        Err(_) => return UrlClass::Web,
    };

    // YouTube hosts (lowercase compare; reject empty host)
    if let Some(host) = parsed.host_str() {
        let host_l = host.to_ascii_lowercase();
        if matches!(
            host_l.as_str(),
            "youtube.com"
                | "www.youtube.com"
                | "youtu.be"
                | "www.youtu.be"
                | "m.youtube.com"
                | "music.youtube.com"
        ) {
            return UrlClass::YouTube;
        }
    }

    // PDF: path ends in .pdf (case-insensitive, ignore query/fragment)
    let path = parsed.path();
    if path.to_ascii_lowercase().ends_with(".pdf") {
        return UrlClass::Pdf;
    }

    UrlClass::Web
}

/// D-03 mid-fetch reroute predicate. Called by Plan 09's local backend after the GET response
/// arrives — when `true`, the response bytes should be handed to the PDF handler instead of
/// the HTML extractor. Matches `application/pdf` (case-insensitive), tolerates parameters
/// like `application/pdf; charset=binary`.
pub fn reroute_for_pdf(content_type_header: &str) -> bool {
    let primary = content_type_header.split(';').next().unwrap_or("").trim();
    primary.eq_ignore_ascii_case("application/pdf")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_youtube_hosts() {
        for url in &[
            "https://youtube.com/watch?v=abc",
            "https://www.youtube.com/watch?v=abc",
            "https://youtu.be/abc",
            "https://m.youtube.com/watch?v=abc",
            "https://music.youtube.com/watch?v=abc",
            "https://YouTube.com/watch?v=abc", // case-insensitive
        ] {
            assert_eq!(classify_url(url), UrlClass::YouTube, "{}", url);
        }
    }

    #[test]
    fn classify_pdf_suffix() {
        for url in &[
            "https://arxiv.org/abs/2401.12345.pdf",
            "https://example.com/doc.PDF",      // case-insensitive
            "https://example.com/doc.pdf?dl=1", // query ignored
            "https://example.com/path/sub/file.pdf#anchor", // fragment ignored
        ] {
            assert_eq!(classify_url(url), UrlClass::Pdf, "{}", url);
        }
    }

    #[test]
    fn classify_default_web() {
        for url in &[
            "https://example.com/article",
            "https://news.ycombinator.com/item?id=1",
            "https://github.com/foo/bar",
            "https://reddit.com/r/rust", // not a YouTube host
        ] {
            assert_eq!(classify_url(url), UrlClass::Web, "{}", url);
        }
    }

    #[test]
    fn classify_unparseable_falls_through_to_web() {
        assert_eq!(classify_url("not a url"), UrlClass::Web);
        assert_eq!(classify_url(""), UrlClass::Web);
    }

    #[test]
    fn classify_does_not_match_youtube_lookalike() {
        // Ensure we don't false-positive on subdomain trickery
        assert_eq!(
            classify_url("https://evil-youtube.com/watch?v=abc"),
            UrlClass::Web
        );
        assert_eq!(
            classify_url("https://youtube.com.evil.example/x"),
            UrlClass::Web
        );
    }

    #[test]
    fn classify_does_not_match_pdf_in_query_string() {
        // `?file=foo.pdf` should NOT trigger PDF route — only path suffix matters
        assert_eq!(
            classify_url("https://example.com/article?file=foo.pdf"),
            UrlClass::Web
        );
    }

    #[test]
    fn reroute_for_pdf_matches_content_type() {
        assert!(reroute_for_pdf("application/pdf"));
        assert!(reroute_for_pdf("application/pdf; charset=binary"));
        assert!(reroute_for_pdf("Application/PDF")); // case-insensitive
        assert!(!reroute_for_pdf("text/html"));
        assert!(!reroute_for_pdf("application/json"));
        assert!(!reroute_for_pdf(""));
    }

    #[test]
    fn backend_name_strings() {
        assert_eq!(Backend::Firecrawl.name(), "firecrawl");
        assert_eq!(Backend::Exa.name(), "exa");
        assert_eq!(Backend::Tavily.name(), "tavily");
        assert_eq!(Backend::Local.name(), "local");
    }
}
