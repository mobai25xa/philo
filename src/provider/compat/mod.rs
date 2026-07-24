//! Typed, composable provider compatibility policies.

mod history;
mod merge;
mod profile;
mod request;
mod response;
mod routing;
mod validate;

pub use history::HistoryCompat;
pub use merge::{CompatPatch, resolve_compat};
pub use profile::{CompatField, CompatProfile};
pub use request::{MaxOutputTokensWireFormat, ModelBodyWireFormat, RequestCompat};
pub use response::{
    FinishReasonCompat, InlineErrorCompat, ResponseCompat, ToolArgumentsCompat, UsageCompat,
};
pub use routing::{
    ConstraintStrength, DataRetention, FallbackDimension, OpenRouterRoutingContract,
    OpenRouterRoutingPatch, ProviderRequestOptions, ResolvedProviderRouting, RoutingFallback,
    RoutingField, RoutingRegion, RoutingSort, UpstreamId,
};
pub use validate::validate_compat;
