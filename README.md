# philo

`philo` is an experimental, streaming-first Rust SDK for LLM applications.

The official OpenAI Chat Completions adapter implements the frozen phase-one text path and the phase-two official semantics for function tools, tool streams/results, history normalization, image inputs, reasoning effort/usage, structured output, and local cost estimation. Third-party Provider profiles, automatic retry, and arbitrary request-body extensions remain out of scope.

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

User guides live under `docs/philo/guide/`.

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
- the official smoke test reads credentials only from `OPENAI_API_KEY`, is disabled
  by default, and never prints prompt, output, secret, or request-ID values.

See [`SECURITY.md`](./SECURITY.md) for reporting and handling expectations.

## Limitations

The official adapter still does not support audio, prompt-cache controls,
third-party Provider profiles, the Responses API, automatic retry, arbitrary
`extra_body`, dangerous header overrides, or automatic tool execution. Visible
thinking text and opaque reasoning signatures are not produced by Official Chat
Completions in phase two; synthetic phase-three boundary fixtures only prove
replay safety. Unsupported or unknown capabilities fail closed.

## License

MIT. See [`LICENSE`](./LICENSE).
