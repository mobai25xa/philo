use std::collections::BTreeMap;

use crate::support::provider::TestProvider;
use philo::ProviderRuntime;
use philo::provider::profiles::OfficialOpenAiProfile;
use philo_presets::{
    DeepSeekProfile, OpenRouterAttribution, OpenRouterProfile, ZaiCodingProfile, ZaiStandardProfile,
};

/// Runtime preset used by the shared offline runner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConformanceProfile {
    OfficialOpenAi,
    OpenRouter,
    DeepSeek,
    ZaiStandard,
    ZaiCoding,
    TestOnly,
}

impl ConformanceProfile {
    pub fn build(self, credential: &str) -> ProviderRuntime {
        match self {
            Self::OfficialOpenAi => OfficialOpenAiProfile::from_api_key(credential)
                .unwrap()
                .build()
                .unwrap(),
            Self::OpenRouter => OpenRouterProfile::from_api_key(credential)
                .unwrap()
                .with_attribution(
                    OpenRouterAttribution::new("https://philo.example", "philo conformance")
                        .unwrap()
                        .with_categories(["sdk", "conformance"])
                        .unwrap(),
                )
                .build()
                .unwrap(),
            Self::DeepSeek => DeepSeekProfile::from_api_key(credential)
                .unwrap()
                .build()
                .unwrap(),
            Self::ZaiStandard => ZaiStandardProfile::from_api_key(credential)
                .unwrap()
                .with_accept_language("en-US")
                .unwrap()
                .build()
                .unwrap(),
            Self::ZaiCoding => ZaiCodingProfile::from_api_key(credential)
                .unwrap()
                .with_accept_language("en-US")
                .unwrap()
                .build()
                .unwrap(),
            Self::TestOnly => {
                TestProvider::new("https://test.invalid/v1/chat/completions", credential)
                    .unwrap()
                    .build()
                    .unwrap()
            }
        }
    }
}

/// Uniform online conformance case names.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum OnlineCase {
    TextStream,
    UsageAndRequestId,
    SingleTool,
    ParallelTool,
    StrictTool,
    JsonObject,
    JsonSchema,
    ImageUrl,
    ImageData,
    ThinkingAndReplay,
    ControlledAuthenticationError,
    Cancellation,
    Timeout,
}

impl OnlineCase {
    pub const ALL: [Self; 13] = [
        Self::TextStream,
        Self::UsageAndRequestId,
        Self::SingleTool,
        Self::ParallelTool,
        Self::StrictTool,
        Self::JsonObject,
        Self::JsonSchema,
        Self::ImageUrl,
        Self::ImageData,
        Self::ThinkingAndReplay,
        Self::ControlledAuthenticationError,
        Self::Cancellation,
        Self::Timeout,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::TextStream => "text_stream",
            Self::UsageAndRequestId => "usage_and_request_id",
            Self::SingleTool => "single_tool",
            Self::ParallelTool => "parallel_tool",
            Self::StrictTool => "strict_tool",
            Self::JsonObject => "json_object",
            Self::JsonSchema => "json_schema",
            Self::ImageUrl => "image_url",
            Self::ImageData => "image_data",
            Self::ThinkingAndReplay => "thinking_and_replay",
            Self::ControlledAuthenticationError => "controlled_401",
            Self::Cancellation => "cancellation",
            Self::Timeout => "timeout",
        }
    }
}

/// Capability state declared by an exact descriptor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CapabilityDeclaration {
    Supported,
    Experimental,
    Unsupported,
    Unknown,
}

/// Whether one online case is required, conditional, or explicitly skipped.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OnlineRequirement {
    Required,
    Conditional,
    Skipped(&'static str),
}

/// Value-free descriptor shared by offline and protected-online runners.
#[derive(Clone, Debug)]
pub struct ConformanceCase {
    pub id: &'static str,
    pub workflow_id: &'static str,
    pub provider: &'static str,
    pub product: &'static str,
    pub exact_model: &'static str,
    pub profile_version: &'static str,
    pub catalog_version: &'static str,
    pub compat_version: &'static str,
    pub endpoint_shape: &'static str,
    pub expected_endpoint: &'static str,
    pub auth_shape: &'static str,
    pub header_shape: &'static str,
    pub expected_headers: &'static [(&'static str, &'static str)],
    pub profile: ConformanceProfile,
    pub capabilities: BTreeMap<OnlineCase, CapabilityDeclaration>,
    pub online: BTreeMap<OnlineCase, OnlineRequirement>,
    pub fixture_manifest: &'static str,
    pub request_id_expected: bool,
    pub generation_id_expected: bool,
    pub usage_expected: bool,
    pub source_kind: &'static str,
    pub reviewed_at: &'static str,
    pub evidence_expires_at: &'static str,
}

pub fn conformance_cases() -> Vec<ConformanceCase> {
    vec![
        official(),
        openrouter(),
        deepseek(),
        zai_standard(),
        zai_coding(),
        test_only(),
    ]
}

fn official() -> ConformanceCase {
    let mut capabilities = BTreeMap::new();
    let mut online = BTreeMap::new();
    for case in OnlineCase::ALL {
        let (capability, requirement) = match case {
            OnlineCase::TextStream
            | OnlineCase::UsageAndRequestId
            | OnlineCase::ControlledAuthenticationError
            | OnlineCase::Cancellation
            | OnlineCase::Timeout => (
                CapabilityDeclaration::Supported,
                OnlineRequirement::Required,
            ),
            _ => (
                CapabilityDeclaration::Unknown,
                OnlineRequirement::Conditional,
            ),
        };
        capabilities.insert(case, capability);
        online.insert(case, requirement);
    }
    ConformanceCase {
        id: "official-openai-chat",
        workflow_id: "official-openai",
        provider: "official-openai",
        product: "chat-completions",
        exact_model: "workflow-input-required",
        profile_version: "3.0.0",
        catalog_version: "synthetic-default",
        compat_version: "openai-chat-default-v1",
        endpoint_shape: "official-https-exact-origin",
        expected_endpoint: "https://api.openai.com/v1/chat/completions",
        auth_shape: "bearer-header",
        header_shape: "json-sse-client-identity",
        expected_headers: &[],
        profile: ConformanceProfile::OfficialOpenAi,
        capabilities,
        online,
        fixture_manifest: "provider/compat/official-openai/manifest.toml",
        request_id_expected: true,
        generation_id_expected: true,
        usage_expected: true,
        source_kind: "synthetic-plus-protected-online",
        reviewed_at: "2026-07-24",
        evidence_expires_at: "2026-10-24",
    }
}

#[allow(clippy::too_many_arguments)]
fn third_party(
    id: &'static str,
    workflow_id: &'static str,
    provider: &'static str,
    product: &'static str,
    model: &'static str,
    profile: ConformanceProfile,
    endpoint: &'static str,
    fixture_manifest: &'static str,
    expected_headers: &'static [(&'static str, &'static str)],
    request_id_expected: bool,
    generation_id_expected: bool,
) -> ConformanceCase {
    let mut capabilities = BTreeMap::new();
    let mut online = BTreeMap::new();
    for case in OnlineCase::ALL {
        let (capability, requirement) = match case {
            OnlineCase::TextStream | OnlineCase::UsageAndRequestId => (
                CapabilityDeclaration::Experimental,
                OnlineRequirement::Required,
            ),
            OnlineCase::ControlledAuthenticationError
            | OnlineCase::Cancellation
            | OnlineCase::Timeout => (
                CapabilityDeclaration::Experimental,
                OnlineRequirement::Conditional,
            ),
            _ => (
                CapabilityDeclaration::Unknown,
                OnlineRequirement::Conditional,
            ),
        };
        capabilities.insert(case, capability);
        online.insert(case, requirement);
    }
    ConformanceCase {
        id,
        workflow_id,
        provider,
        product,
        exact_model: model,
        profile_version: "3.0.0-experimental",
        catalog_version: "provider-catalog-reviewed-2026-07-23",
        compat_version: "openai-chat-compatible-v1",
        endpoint_shape: "provider-https-exact-product",
        expected_endpoint: endpoint,
        auth_shape: "bearer-header-real-provider-target",
        header_shape: "json-sse-client-identity-provider-allowlist",
        expected_headers,
        profile,
        capabilities,
        online,
        fixture_manifest,
        request_id_expected,
        generation_id_expected,
        usage_expected: true,
        source_kind: "official-doc-plus-synthetic-offline",
        reviewed_at: "2026-07-23",
        evidence_expires_at: "2026-10-23",
    }
}

fn openrouter() -> ConformanceCase {
    third_party(
        "openrouter-chat",
        "openrouter",
        "openrouter",
        "openrouter-chat",
        "nvidia/nemotron-3-ultra-550b-a55b:free",
        ConformanceProfile::OpenRouter,
        "https://openrouter.ai/api/v1/chat/completions",
        "provider/compat/openrouter/manifest.toml",
        &[
            ("http-referer", "https://philo.example"),
            ("x-openrouter-title", "philo conformance"),
            ("x-openrouter-categories", "sdk,conformance"),
        ],
        false,
        true,
    )
}

fn deepseek() -> ConformanceCase {
    third_party(
        "deepseek-chat-openai",
        "deepseek",
        "deepseek",
        "deepseek-chat-openai",
        "deepseek-v4-flash",
        ConformanceProfile::DeepSeek,
        "https://api.deepseek.com/chat/completions",
        "provider/compat/deepseek/manifest.toml",
        &[],
        true,
        true,
    )
}

fn zai_standard() -> ConformanceCase {
    third_party(
        "zai-standard-api",
        "zai-standard",
        "zai",
        "zai-standard-api",
        "glm-4.7-flash",
        ConformanceProfile::ZaiStandard,
        "https://api.z.ai/api/paas/v4/chat/completions",
        "provider/compat/zai-standard/manifest.toml",
        &[("accept-language", "en-US")],
        false,
        true,
    )
}

fn zai_coding() -> ConformanceCase {
    third_party(
        "zai-coding-plan",
        "zai-coding",
        "zai",
        "zai-coding-plan",
        "glm-4.7-flash",
        ConformanceProfile::ZaiCoding,
        "https://api.z.ai/api/coding/paas/v4/chat/completions",
        "provider/compat/zai-coding/manifest.toml",
        &[("accept-language", "en-US")],
        false,
        true,
    )
}

fn test_only() -> ConformanceCase {
    let capabilities = OnlineCase::ALL
        .into_iter()
        .map(|case| (case, CapabilityDeclaration::Unsupported))
        .collect();
    let online = OnlineCase::ALL
        .into_iter()
        .map(|case| {
            (
                case,
                OnlineRequirement::Skipped("offline-only profile has no protected online target"),
            )
        })
        .collect();
    ConformanceCase {
        id: "test-only-openai-wire",
        workflow_id: "test-only",
        provider: "test-only",
        product: "chat-completions",
        exact_model: "conformance-test-model",
        profile_version: "3.0.0-test-only",
        catalog_version: "synthetic-default",
        compat_version: "openai-chat-default-v1",
        endpoint_shape: "loopback-http-exact-origin",
        expected_endpoint: "https://test.invalid/v1/chat/completions",
        auth_shape: "bearer-header-test-only",
        header_shape: "json-sse-client-identity",
        expected_headers: &[],
        profile: ConformanceProfile::TestOnly,
        capabilities,
        online,
        fixture_manifest: "provider/compat/test-only/manifest.toml",
        request_id_expected: false,
        generation_id_expected: false,
        usage_expected: true,
        source_kind: "synthetic",
        reviewed_at: "2026-07-24",
        evidence_expires_at: "2027-07-24",
    }
}
