# philo

`philo` is an experimental, streaming-first Rust SDK for multi-provider LLM applications.

Phase one is intentionally narrow: native Rust with Tokio, the official OpenAI Chat Completions endpoint, Bearer authentication, text messages, and SSE streaming. Tools, reasoning, images, third-party Provider profiles, automatic retry, and arbitrary request-body extensions are not yet supported.

The behavior contract is `philo/openai-chat-p1` version `1.0.0`. Architecture and implementation planning are maintained alongside the SDK project; the crate remains independently buildable after clone.

## Status

The crate is `0.x` and its Rust API is experimental. The phase-one protocol behavior is frozen; API changes before 1.0 are documented in release notes.

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
- ordinary request overrides will not be allowed to modify protected headers;
- tests are offline unless an explicit smoke workflow is enabled in a later task.

See [`SECURITY.md`](./SECURITY.md) for reporting and handling expectations.

## License

MIT. See [`LICENSE`](./LICENSE).
