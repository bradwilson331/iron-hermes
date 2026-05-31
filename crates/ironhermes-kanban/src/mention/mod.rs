pub mod parser;
pub mod resolver;

pub use parser::{parse_mentions, MentionSpan};
pub use resolver::{
    resolve_mention, FallbackPolicy, FallbackReason, Resolution,
    ResolverCtx, SkipReason, MAX_MENTION_CHAIN_DEPTH,
};
