//! Built-in provider presets.
//!
//! Every preset is an ordinary caller of [`ProviderDefinition`](super::ProviderDefinition):
//! it fixes an endpoint, a credential audience, a protocol contract, and a model
//! catalog, then compiles a runtime. Nothing here is reachable by the generic
//! pipeline — presets depend on the core, never the reverse.

mod official_anthropic;
mod official_openai;
#[cfg(test)]
mod test_only;

pub use official_anthropic::{OFFICIAL_ANTHROPIC_API_VERSION, OfficialAnthropicProfile};
pub use official_openai::OfficialOpenAiProfile;
#[cfg(test)]
pub(crate) use test_only::TestProvider;
