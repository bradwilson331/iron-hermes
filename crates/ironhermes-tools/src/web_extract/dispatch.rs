//! Phase 25.2 D-03: URL classification + backend selection.
//!
//! All functions are pure (no network, no env I/O except `select_backend` which reads env vars).
//! Classification runs BEFORE any HTTP request per D-03.

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

/// Backend selected for the default web branch, based on env-var presence.
/// Order is fixed: Firecrawl > Exa > Tavily > Local.
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

/// D-04 backend chain selector. Reads env vars at call time (cheap; matches `web_read.rs:550`
/// pattern). Returns `Backend::Local` when no provider env var is set.
pub fn select_backend() -> Backend {
    if std::env::var("FIRECRAWL_API_KEY").is_ok() {
        Backend::Firecrawl
    } else if std::env::var("EXA_API_KEY").is_ok() {
        Backend::Exa
    } else if std::env::var("TAVILY_API_KEY").is_ok() {
        Backend::Tavily
    } else {
        Backend::Local
    }
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

    // select_backend tests are env-var sensitive — defer to integration tests in Plan 14
    // which use env_lock + EnvGuard. Unit-testing it here would race with other tests.
}
