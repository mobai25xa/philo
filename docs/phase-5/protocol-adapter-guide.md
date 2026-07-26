# Adding a protocol adapter

A protocol adapter translates one validated domain call into request bytes and
one response byte stream into ordered domain events. It does not own secrets,
endpoint resolution, HTTP, retry, timeout, cancellation, observability, Tool
execution, remote image fetching, or cross-provider fallback.

## Required sequence

1. Freeze the official protocol version, endpoint/auth contract, required
   capabilities, unsupported features, and independent fixtures.
2. Record a semantic matrix against both existing protocols. Classify each item
   as Equivalent, Transformable, Lossy, ProtocolOnly, or Unsupported.
3. Add a private typed wire module and a private per-request response state
   machine. Reuse only shared framing and execution contracts.
4. Add an explicit `ProtocolDialect` dispatch arm and provider/model catalog
   declaration. Never infer protocol from endpoint, model name, or JSON shape.
5. Keep common intent in Domain. Add a typed protocol option only for a stable,
   useful protocol-only capability. Keep any raw extension bounded, protected,
   dangerous-by-name, diagnostic, and value-free in logs.
6. Add independent request/stream fixtures, arbitrary fragmentation tests,
   resource-limit tests, redaction canaries, and common-domain conformance tests.
7. Extend architecture fitness so adapters cannot import each other's wire
   modules, create network clients, execute Tools, or leak wire types publicly.
8. Review the public API diff and document behavior, migration, security, and
   unsupported semantics before adding a real-provider smoke target.

The adapter must emit exactly one normal `Done`, only after its protocol's real
terminal signal. EOF, a stream error, illegal state transition, incomplete Tool
JSON, or unknown unsafe terminal semantics must fail closed.

## Review checklist

- Domain has no provider wire names or wire types.
- Provider auth/header/endpoint policies use the shared pipeline.
- Every request, SSE, block, text, image, Tool JSON, and opaque accumulator has a
  typed limit.
- `stream()` and `complete()` use the existing lifecycle.
- Unsupported capabilities fail before transport with an explicit reason.
- Fixture claims and real-provider claims remain separate.
- SDK boundaries exclude Tool execution, Agent loops, URL downloads, persistence,
  billing, gateway limiting, and fallback.
