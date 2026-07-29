# philo

`philo` is a secure, streaming-first Rust SDK for multi-provider LLM
applications. It exposes provider-independent requests and events while keeping
protocol wire types, credentials, retries, and network policy behind typed
boundaries.

## Stability

The crate is currently a pre-1.0 release candidate. The intended 1.0 boundary is:

| Class | Surface |
|---|---|
| Stable candidate | Core request/event API, tools and history, OpenAI Chat Completions, reliability and security behavior |
| Experimental | Anthropic Messages until same-candidate Canary passes, `philo-config`, `philo-presets`, protocol-specific thinking, and the `tracing` adapter |
| Escape Hatch | Bounded raw OpenAI and Anthropic top-level body extensions; safety is Stable, provider semantics are not portable |
| Not in 1.0 | OpenAI Responses, agent loops, MCP or Tool execution, cross-provider fallback, and custom protocol ABI |

Unknown model capabilities fail closed. The SDK validates Tool calls but never
authorizes or executes them.

See [COMPATIBILITY.md](./COMPATIBILITY.md) for the SemVer, MSRV, deprecation,
hotfix, and yank policy. Pre-1.0 breaking changes are listed in
[CHANGELOG.md](./CHANGELOG.md).

## Minimal Call

```rust,no_run
use philo::provider::profiles::OfficialOpenAiProfile;
use philo::{GenerateRequest, LlmClient, Message, ModelRef};

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

Applications provide the Tokio runtime. The library never creates one.

## Providers

- Official OpenAI: construct `OfficialOpenAiProfile` directly.
- Official Anthropic: use `ProviderRegistry::with_official_anthropic`; it remains
  Experimental until protected same-candidate verification passes.
- Third-party products: use the independently versioned Experimental
  `philo-presets` crate and check the machine-readable
  [support registry](./support/provider-support-matrix.toml) plus the
  [current limitations](./support/provider-limitations.md).
- Custom HTTPS origins: build an explicit `ProviderDefinition`; provider identity
  never selects a protocol adapter.

The maintained examples are:

- [`quickstart`](./examples/quickstart.rs): a minimal streaming OpenAI request;
- [`tool_single`](./examples/tool_single.rs): application-owned Tool validation,
  execution, result history, and follow-up;
- [`cancellation_timeout`](./examples/cancellation_timeout.rs): deadline and
  caller cancellation;
- [`anthropic_messages`](./examples/anthropic_messages.rs): typed Anthropic
  Messages options;
- [`custom_provider`](./examples/custom_provider.rs): a caller-declared HTTPS
  OpenAI-compatible origin;
- [`provider_profiles`](./examples/provider_profiles.rs): offline construction of
  Experimental third-party presets;
- [`raw_extension`](./examples/raw_extension.rs): bounded, protected,
  non-portable body extension.

All examples compile in ordinary CI. Live examples require their documented
environment variables and otherwise avoid network calls where possible.

## Security

- rustls is the default TLS implementation and unsafe Rust is forbidden;
- official credentials are bound to their exact HTTPS origin;
- redirects are disabled for official profiles;
- Authorization, Host, Content-Length, Content-Type, and connection headers are
  protected from ordinary overrides;
- error bodies, raw extensions, retries, streams, and history are bounded;
- default diagnostics contain no prompts, outputs, secrets, header values, or
  complete HTTP bodies.

Report vulnerabilities through the hosting platform's private security-reporting
channel. Do not place credentials, active payloads, prompts, or model output in a
public issue. See [SECURITY.md](./SECURITY.md) for the complete boundary.

## Migration

The candidate removes alpha-only compatibility and test-support crates, public
stage-numbered constants, public mock transports, and migration-only public
paths. Use the capability-named constants and repository-private test support.
No automatic Tool execution, Responses API, or custom protocol replacement is
provided because those capabilities are outside the 1.0 boundary.

## Development

```text
cargo fmt --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
cargo test --workspace --all-targets --no-default-features
```

Release and compatibility ownership is defined in
[`docs/maintenance`](./docs/maintenance/README.md). Provider status is owned by
the machine-readable
[`support/provider-support-matrix.toml`](./support/provider-support-matrix.toml).

## License

MIT. See [LICENSE](./LICENSE).
