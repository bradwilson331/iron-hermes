pub mod parser;
pub mod resolver;

pub use parser::{MentionSpan, parse_mentions};
pub use resolver::{
    FallbackPolicy, FallbackReason, MAX_MENTION_CHAIN_DEPTH, Resolution, ResolverCtx, SkipReason,
    resolve_mention,
};
