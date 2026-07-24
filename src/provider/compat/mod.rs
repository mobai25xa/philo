//! Typed, composable provider compatibility policies.

mod history;
mod merge;
mod profile;
mod request;
mod response;
mod validate;

pub use history::HistoryCompat;
pub use merge::{CompatPatch, resolve_compat};
pub use profile::{CompatField, CompatProfile};
pub use request::{MaxOutputTokensWireFormat, RequestCompat};
pub use response::{
    FinishReasonCompat, InlineErrorCompat, ResponseCompat, ToolArgumentsCompat, UsageCompat,
};
pub use validate::validate_compat;
