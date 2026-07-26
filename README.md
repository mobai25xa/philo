# philo

`philo` is an experimental, streaming-first Rust SDK for LLM applications.

The official OpenAI Chat Completions adapter implements the frozen phase-one text path and the phase-two official semantics for function tools, tool streams/results, history normalization, image inputs, reasoning effort/usage, structured output, and local cost estimation. Phase three adds experimental OpenRouter, DeepSeek, and Z.AI profiles plus versioned configuration, typed compatibility policy, deployment mapping, diagnostics, and offline conformance. Phase four adds bounded deadlines, stage timeouts, opt-in same-provider retry, backpressure, typed rate/idempotency metadata, secure network policy, and value-free lifecycle hooks. Arbitrary request-body extensions remain out of scope.

Phase five adds official Anthropic Messages as a second protocol behind the same
client and reliability pipeline. Anthropic request/stream wire types remain
private; typed protocol options expose adaptive thinking and effort without
adding provider fields to the common domain.

Behavior contracts:

- `philo/openai-chat-p1` version `1.0.0` for the text stream foundation
- `philo/openai-chat-p2` version `1.1.0` for tools, multimodal, structured output, related domain types, and the call-planning contract

Architecture and implementation planning are maintained alongside the SDK project; the crate remains independently buildable after clone.

## Status

The crate is `0.x` and its Rust API is experimental. Phase-one and phase-two protocol behavior are frozen; API changes before 1.0 are documented in release notes.

The current implementation includes:

- provider-independent multi-content parts, tools, history normalizer, usage/cost helpers;
- ordered Domain v2 events and collectors, including tool-call and structured-output completion;
- exact `ModelId` capability profiles and official OpenAI runtime;
- public `LlmClient::stream/complete` entry points that still consume one stream only;
- typed errors/identifiers and value-free lifecycle observations;
- rustls-backed transport with fail-closed headers/auth/body limits.
- experimental exact-product profiles for OpenRouter, DeepSeek, Z.AI standard,
  and Z.AI coding, all currently backed by offline contract evidence only;
- value-free provider diagnostics and a five-state support vocabulary;
- one absolute deadline across attempts, layered timeouts, cancellable full-jitter
  waits, and a permanent no-retry boundary after the first delivered event;
- an optional `tracing` lifecycle adapter without an SDK-owned subscriber/exporter;
- offline benchmark, fault-matrix, and per-client soak harnesses.
- official Anthropic Messages text, Tool, thinking, image, usage, finish, and
  error mapping with explicit protocol selection and bounded raw extensions.

Capabilities that are `Unknown` for an exact model id fail closed. The SDK never executes tools; applications validate, authorize, and run them.

## Minimal Complete Call

```rust,no_run
use philo::{GenerateRequest, LlmClient, Message, ModelRef, OfficialOpenAiProfile};

# async fn run() -> Result<(), philo::LlmError> {
let key = std::env::var("OPENAI_API_KEY")
    .map_err(|_| philo::LlmError::Configuration("OPENAI_API_KEY is required".into()))?;
let model = std::env::var("OPENAI_MODEL")
    .map_err(|_| philo::LlmError::Configuration("OPENAI_MODEL is required".into()))?;
let runtime = OfficialOpenAiProfile::from_api_key(key)?.build()?;
let client = LlmClient::with_reqwest(runtime)?;
let request = GenerateRequest::new(
    ModelRef::new("official-openai", model)?,
    vec![Message::user("Reply briefly.")],
);
let message = client.complete(request).await?;
println!("{}", message.text());
# Ok(())
# }
```

Examples:

- phase one: `stream_text.rs`, `complete_text.rs`, `cancellation_timeout.rs`, `request_headers.rs`, `typed_errors.rs`
- phase two: `tool_single.rs`, `tool_parallel.rs`, `tool_reject.rs`, `image_url.rs`, `structured_json_schema.rs`
- phase three: `provider_profiles.rs`, `provider_diagnostics.rs`,
  `provider_auth_shapes.rs`, `provider_config.rs`, `deployment_mapping.rs`,
  `provider_routing.rs`
- phase four: `reliability_controls.rs`, `lifecycle_observer.rs`,
  and `slow_consumer_drop.rs`; the offline soak lives in
  `benches/phase4_client_soak.rs`
- phase five: `anthropic_messages.rs`

Provider guides live in the workspace under `docs/philo/stage/guide/providers/`.
The standalone repository keeps its checked support declaration in
[`support/provider-support-matrix.md`](./support/provider-support-matrix.md),
with [`provider-support-matrix.toml`](./support/provider-support-matrix.toml) as
the machine-readable source of truth.

Reliability guidance:

- [`support/reliability-configuration.md`](./support/reliability-configuration.md)
- [`support/reliability-troubleshooting.md`](./support/reliability-troubleshooting.md)
- [`support/security-boundary.md`](./support/security-boundary.md)
- [`support/phase4-migration.md`](./support/phase4-migration.md)
- [`docs/phase-4/README.md`](./docs/phase-4/README.md)
- [`docs/phase-5/README.md`](./docs/phase-5/README.md)
- [`support/phase5-support-matrix.md`](./support/phase5-support-matrix.md)

## Build

From the `philo` repository root:

```text
cargo test
```

Supported feature checks:

```text
cargo check --no-default-features
cargo check --all-features
```

The library never creates a Tokio runtime. Applications provide the runtime.

## Metadata Example

```rust
use philo::{PHASE_ONE_CONTRACT_ID, PHASE_TWO_CONTRACT_ID, SDK_NAME};

assert_eq!(SDK_NAME, "philo");
assert_eq!(PHASE_ONE_CONTRACT_ID, "philo/openai-chat-p1");
assert_eq!(PHASE_TWO_CONTRACT_ID, "philo/openai-chat-p2");
```

## Security Defaults

- rustls is the default TLS backend;
- unsafe Rust is forbidden in this crate;
- secrets, prompts, outputs, and complete HTTP bodies are not logged by default;
- default tests are offline; the official smoke workflow is separate and opt-in;
- ordinary request overrides cannot modify Authorization, Host, Content-Length,
  Content-Type, or connection-specific headers;
- redirects are disabled by the official phase-one profile;
- HTTP error bodies are bounded and redacted before entering diagnostics;
- official smoke tests read only their documented named credential variables,
  are disabled by default, and never print prompt, output, secret, Tool arguments,
  thinking, or request-ID values.

See [`SECURITY.md`](./SECURITY.md) for reporting and handling expectations.

## Limitations

The SDK still does not support audio, prompt-cache automation, the Responses API,
cross-provider fallback, dangerous header overrides, or automatic Tool execution.
Anthropic raw extensions are explicit, bounded, protected, non-portable, and
diagnostic; OpenAI has no arbitrary body extension. Unsupported or unknown
capabilities fail closed.

OpenRouter, DeepSeek, and Z.AI profiles remain `Experimental`: offline fixtures
do not constitute real-provider verification or a `Supported` claim. Their
protected online runs, hosted exact-SHA evidence, and independent reviews are
still pending.

## License

MIT. See [`LICENSE`](./LICENSE).
