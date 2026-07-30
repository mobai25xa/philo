# SDK and deployment security boundary

The SDK owns temporary in-process request/response data, typed errors, Debug
implementations, lifecycle events, retry/network limits, and the safety of its
examples. It separates provider credentials, proxy credentials, custom trust
roots, and client identity material. Protected authentication, idempotency, Host,
Content-Length, Content-Type, and connection headers cannot be forged by ordinary
request overrides.

The application or deployment owns tool authorization and transactions, tenant
data policy, tracing/metrics exporter credentials and endpoints, retention and
deletion, crash dumps, process/host security, and any content it explicitly logs.
Native observer callbacks are trusted application code: panics are isolated, but
deliberate blocking cannot be preempted by the SDK.

Default diagnostics do not contain prompt/output/tool/thinking content, Header
values, error bodies, complete URLs, idempotency keys, or secrets. Identifiers may
be used for trace correlation but must not be metrics labels. New content logging
requires a separate opt-in API, ADR, and security review; the stable SDK provides none.
