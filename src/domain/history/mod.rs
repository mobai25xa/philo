//! History normalization, tool pairing, and replay boundaries.

mod diagnostics;
mod normalize;
mod policy;
mod replay;

pub use diagnostics::{DiagnosticCode, IdMapping, NormalizationDiagnostic, NormalizedContext};
pub use normalize::normalize_history;
pub(crate) use normalize::normalize_history_with_limits;
pub use policy::{
    DialectPolicy, HistoryCapabilities, HistoryPolicy, ImageWireFormat, MissingToolResultPolicy,
    PolicySource, StreamUsagePolicy, StructuredOutputWireFormat, ThinkingReplayPolicy,
    ThinkingWireFormat, ToolCallIdPolicy, ToolChoiceWireFormat, ToolResultNamePolicy,
    UnsupportedContentPolicy,
};
pub use replay::{apply_thinking_replay_policy, drop_opaque_reasoning};
