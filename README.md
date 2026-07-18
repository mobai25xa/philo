# philo

`philo` is an experimental, streaming-first Rust SDK for LLM applications.

Phase one is intentionally narrow: native Rust with Tokio, the official OpenAI Chat Completions endpoint, Bearer authentication, text messages, and SSE streaming. Tools, reasoning, images, third-party Provider profiles, automatic retry, and arbitrary request-body extensions are not yet supported.

The behavior contract is `philo/openai-chat-p1` version `1.0.0`. Architecture and implementation planning are maintained alongside the SDK project; the crate remains independently buildable after clone.

## Status

The crate is `0.x` and its Rust API is experimental. The phase-one protocol behavior is frozen; API changes before 1.0 are documented in release notes.

The current implementation includes provider-independent request/event types, the official OpenAI provider runtime, public `LlmClient::stream/complete` entry points, typed errors and identifiers, structured value-free lifecycle observations, a private Chat request/response protocol implementation, and a rustls-backed transport.

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

See `examples/stream_text.rs`, `complete_text.rs`, `cancellation_timeout.rs`,
`request_headers.rs`, and `typed_errors.rs` for complete public-API usage.

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
use philo::{PHASE_ONE_CONTRACT_ID, SDK_NAME};

assert_eq!(SDK_NAME, "philo");
assert_eq!(PHASE_ONE_CONTRACT_ID, "philo/openai-chat-p1");
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

Phase one does not support tools/function calls, thinking/reasoning controls,
images, audio, structured outputs, prompt-cache controls, third-party Provider
profiles, the Responses API, automatic retry, arbitrary `extra_body`, or dangerous
header overrides. Unsupported or unknown response semantics fail closed.

## License

MIT. See [`LICENSE`](./LICENSE).
