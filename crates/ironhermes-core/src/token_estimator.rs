// Token estimation using tiktoken-rs BPE singletons.
// Phase 21.3 — replaces text.len()/4+1 heuristic with accurate BPE tokenization.

use std::sync::OnceLock;
use tiktoken_rs::{cl100k_base_singleton, o200k_base_singleton};

/// Supported tiktoken BPE encodings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TiktokenEncoding {
    Cl100kBase,
    O200kBase,
}

impl TiktokenEncoding {
    /// Map a tiktoken encoding name string to the enum variant.
    /// Fallback to Cl100kBase for unknown encodings (D-08).
    pub fn from_name(name: &str) -> Self {
        match name {
            "o200k_base" => Self::O200kBase,
            _ => Self::Cl100kBase,
        }
    }
}

/// Token estimator backed by tiktoken-rs BPE singletons.
/// Thread-safe: singletons return `&'static CoreBPE` references (lazy_static).
pub struct TokenEstimator {
    encoding: TiktokenEncoding,
}

impl TokenEstimator {
    pub fn new(encoding: TiktokenEncoding) -> Self {
        Self { encoding }
    }

    /// Count tokens in the given text using BPE tokenization.
    pub fn count(&self, text: &str) -> usize {
        if text.is_empty() {
            return 0;
        }
        match self.encoding {
            TiktokenEncoding::O200kBase => o200k_base_singleton()
                .encode_with_special_tokens(text)
                .len(),
            TiktokenEncoding::Cl100kBase => cl100k_base_singleton()
                .encode_with_special_tokens(text)
                .len(),
        }
    }

    /// Returns the encoding used by this estimator.
    pub fn encoding(&self) -> TiktokenEncoding {
        self.encoding
    }
}

static GLOBAL_ESTIMATOR: OnceLock<TokenEstimator> = OnceLock::new();

/// Initialize the global token estimator from the model's encoding name.
/// Called once at startup after model metadata is resolved.
/// Safe to call multiple times -- only the first call sets the value.
pub fn init_global_estimator(encoding: TiktokenEncoding) {
    GLOBAL_ESTIMATOR.get_or_init(|| TokenEstimator::new(encoding));
}

/// Count tokens using the global estimator.
/// Falls back to text.len()/4+1 if the global estimator hasn't been initialized yet.
pub fn global_estimate_tokens(text: &str) -> usize {
    GLOBAL_ESTIMATOR
        .get()
        .map(|e| e.count(text))
        .unwrap_or_else(|| text.len() / 4 + 1)
}

/// Eagerly initialize both tiktoken BPE tables.
/// Call during startup (before async runtime) to avoid ~100ms latency on first token count.
pub fn warm_tiktoken_singletons() {
    let _ = cl100k_base_singleton();
    let _ = o200k_base_singleton();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn count_cl100k_hello_world() {
        let est = TokenEstimator::new(TiktokenEncoding::Cl100kBase);
        let count = est.count("hello world");
        assert!(count > 0, "token count should be nonzero");
    }

    #[test]
    fn count_o200k_hello_world() {
        let est = TokenEstimator::new(TiktokenEncoding::O200kBase);
        let count = est.count("hello world");
        assert!(count > 0, "token count should be nonzero");
    }

    #[test]
    fn count_empty_string_returns_zero() {
        let est = TokenEstimator::new(TiktokenEncoding::Cl100kBase);
        assert_eq!(est.count(""), 0);
    }

    #[test]
    fn from_name_cl100k() {
        assert_eq!(
            TiktokenEncoding::from_name("cl100k_base"),
            TiktokenEncoding::Cl100kBase
        );
    }

    #[test]
    fn from_name_o200k() {
        assert_eq!(
            TiktokenEncoding::from_name("o200k_base"),
            TiktokenEncoding::O200kBase
        );
    }

    #[test]
    fn from_name_unknown_falls_back_to_cl100k() {
        assert_eq!(
            TiktokenEncoding::from_name("unknown_encoding"),
            TiktokenEncoding::Cl100kBase
        );
    }

    #[test]
    fn count_differs_from_heuristic() {
        // A long string where BPE and len/4+1 should differ, proving BPE is active
        let text = "The quick brown fox jumps over the lazy dog. ".repeat(50);
        let est = TokenEstimator::new(TiktokenEncoding::Cl100kBase);
        let bpe_count = est.count(&text);
        let heuristic = text.len() / 4 + 1;
        assert_ne!(
            bpe_count, heuristic,
            "BPE count ({bpe_count}) should differ from heuristic ({heuristic})"
        );
    }

    #[test]
    fn global_estimator_init_and_delegate() {
        // Note: OnceLock means only the first call sets the value, so this test
        // may interact with other tests if run in the same process. We just verify
        // it doesn't panic and returns a sensible value.
        init_global_estimator(TiktokenEncoding::Cl100kBase);
        let count = global_estimate_tokens("hello world");
        assert!(count > 0, "global estimate should be nonzero");
    }
}
