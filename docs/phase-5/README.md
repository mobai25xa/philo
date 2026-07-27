# Phase 5 Anthropic Messages

Phase 5 adds official Anthropic Messages as a second protocol behind the same
`LlmClient`, execution, transport, retry, timeout, cancellation, and observer
pipeline used by OpenAI Chat Completions. Protocol selection is frozen in the
selected `ProviderRuntime`; the SDK does not inspect URLs or request JSON to
guess a protocol and never falls back to another provider or protocol.

## Build an official client

Use a named secret reference so configuration contains no credential value:

```rust,no_run
use philo::ProviderRuntime;
use philo::provider::secret::{EnvironmentSecretResolver, SecretReference};
use philo_config::{ConfigSource, ConfigValue, ProviderConfigLayer, ProviderConfigSnapshot};

# fn build() -> Result<philo::ProviderRuntime, philo::LlmError> {
let credential = ProviderConfigLayer::new(ConfigSource::environment_secret(
    "env/anthropic", "ANTHROPIC_API_KEY",
)?)
.with_credential(ConfigValue::set(SecretReference::environment_variable(
    "ANTHROPIC_API_KEY",
)?));
let config = ProviderConfigSnapshot::official_anthropic()?.merge_layers([credential])?;
let (definition, deployment) = config.official_anthropic_inputs()?;
let profile = definition.compile(&deployment, &EnvironmentSecretResolver)?;
let runtime = ProviderRuntime::build(profile)?;
# Ok(runtime)
# }
```

The runtime selects `anthropic-messages`, the exact
`https://api.anthropic.com/v1/messages` endpoint, `x-api-key` authentication,
and `anthropic-version: 2023-06-01`. Request overrides cannot replace the auth,
version, beta, Host, Content-Length, or content-type owners.

## Send one request

`GenerateRequest` remains provider neutral. Anthropic requires a positive output
limit unless the exact catalog entry supplies a default.

```rust,no_run
use philo::{GenerateRequest, GenerationOptions, Message, ModelRef};

# fn request() -> Result<GenerateRequest, philo::LlmError> {
let request = GenerateRequest::new(
    ModelRef::new("official-anthropic", "claude-sonnet-5")?,
    vec![
        Message::system("Answer briefly."),
        Message::user("Summarize the result."),
    ],
)
.with_options(GenerationOptions::new().with_max_output_tokens(512));
# Ok(request)
# }
```

System and developer instructions become Anthropic's top-level system blocks.
User and assistant history becomes alternating Messages content. The shared
normalizer validates Tool pairing and replay identity before the Anthropic
adapter performs its protocol-specific conversion. Lossy normalization is
reported through normalization diagnostics; it is never silently repaired by
changing the caller's request.

## Common, typed, and raw options

Use common `GenerationOptions` for shared intent. Use
`AnthropicMessagesOptions` for stable Anthropic-only behavior:

```rust
use philo::GenerationOptions;
use philo::protocol_options::{AnthropicEffort, AnthropicMessagesOptions, AnthropicThinkingDisplay};

let options = GenerationOptions::new()
    .with_max_output_tokens(1024)
    .with_protocol_options(
        AnthropicMessagesOptions::new()
            .with_adaptive_thinking(AnthropicThinkingDisplay::Summarized)
            .with_effort(AnthropicEffort::High),
    );
assert!(options.protocol_options().is_some());
```

Typed options are checked against the selected protocol and exact-model
capabilities before transport. Passing Anthropic options to OpenAI fails.
`AnthropicRawExtension::dangerous_from_object` is a bounded, non-portable escape
hatch for unknown top-level body fields. It cannot override core, auth, version,
beta, or header owners. Its values and keys are omitted from `Debug` and errors;
using it emits `NonPortableExtensionUsed`. Promote repeated stable usage to a
typed option instead of normalizing raw JSON in application code.

## Tools, thinking, and images

The SDK encodes Tool definitions and emits complete `ToolCall` values only after
their JSON is valid. The application validates, authorizes, executes, and returns
a `ToolResultMessage`; the SDK does not execute a Tool or automatically continue
an agent loop.

Visible thinking is separate from answer text. Signatures and redacted thinking
remain opaque and carry a `SourceIdentity`; replay is allowed only by the selected
history policy for the same source. Do not log or display opaque values. The SDK
never invents text for redacted thinking.

Supported HTTPS image URLs are encoded as URLs, not downloaded by the SDK. Inline
images are validated and bounded before transport. URL fetching, redirects,
content scanning, and file hosting belong to the application or a dedicated
media service.

## Usage, finish, and errors

OpenAI and Anthropic can expose different accounting completeness. Compare known
`UsageDetails` fields; do not derive Anthropic totals or combine cache/reasoning
categories when the provider did not define that calculation. Known Anthropic
stop reasons map to `Stop`, `Length`, or `ToolCalls`; unknown reasons remain typed
errors or raw unknown values rather than a false success.

HTTP and in-stream errors retain safe status, provider code, retry hint, and
request ID where available. Error body prefixes are bounded and redacted.
`complete()` is still a collector over `stream()` and has no Anthropic-specific
network path.

See [capabilities and limitations](../../support/phase5-support-matrix.md), the
[complete example](../../examples/anthropic_messages.rs), and the
[new protocol adapter guide](./protocol-adapter-guide.md).
