# Reliability configuration

The reliability layer remains an LLM SDK concern. It does not implement agent loops,
tool execution, cross-provider fallback, circuit breakers, tenant queues, or service SLOs.

## Defaults and precedence

`LlmClient` owns immutable defaults. `RequestControl` may only tighten them for one
call. A request's `GenerationOptions` supplies the overall absolute deadline;
stage timeouts and every retry wait are clamped by that same deadline. No attempt
resets it.

| Control | Default |
|---|---:|
| automatic attempts | 1 (retry disabled) |
| `RetryPolicy::standard()` | at most 3 attempts |
| minimum next-attempt budget | 100 ms |
| credential timeout | 10 s |
| response-header timeout | 30 s |
| first-event timeout | 30 s |
| idle-stream timeout | 60 s |
| backoff base / client cap | 100 ms / 5 s |
| server delay cap / total wait cap | 60 s / 60 s |

Use `RetryPolicy::standard()` explicitly. Retry remains within the selected
provider/runtime and rebuilds attempt-local credentials, headers, request ID,
response decoder, and rate metadata. The logical request ID, prepared call,
idempotency key, and absolute deadline remain stable.

## Non-overridable delivery rule

Once any `AssistantEvent` is returned by the public `AssistantStream`, automatic
retry is permanently disabled. This includes Start, text, usage, thinking,
refusal, ToolCallStart/Delta/End, and Done. The SDK never repeats a delivered
domain or tool event.

Idempotency keys reduce duplicate provider submission risk but do not provide
exactly-once model generation or tool side effects. `Unknown` capability fails
closed. Applications remain responsible for idempotent, authorized tool effects.

## Wait and rate metadata

Backoff uses bounded full jitter. Valid `Retry-After` or reviewed provider reset
headers may increase the wait, subject to the server-delay cap, total-wait cap,
minimum next-attempt budget, cancellation, and overall deadline. Raw Header
values are not retained in `RateLimitObservation` or lifecycle events.

## Buffers and network policy

The response pipeline is poll-driven: an unpolled public stream does not read the
body. Default SSE limits include a 1 MiB event, 64 KiB line, 256 KiB upstream
chunk, 128 fields/event, and cooperative per-poll work budgets. Response headers
are limited to 128 fields, 16 KiB per value, and 64 KiB total after HTTP parsing.
Request/history/tool/schema/image/structured-output limits are exposed through
`ResourceLimits` and resolved once during planning.

`NetworkPolicy` defaults to verified rustls TLS, bounded redirects/pools/config,
and separated provider, proxy, and client-identity secrets. Do not disable TLS
verification for development; use a reviewed custom CA or loopback TestOnly
profile. Proxy configuration is explicit and does not implicitly trust ambient
proxy variables.

## Cancellation and observability

Keep a clone of `RequestControl` to cancel header, wait, or body work. Dropping
`AssistantStream` also cancels unfinished work. `complete()` consumes the same
stream; it does not open a second request.

`LifecycleObserver` is synchronous and must not block. Observer panics are
isolated from the request result, but the SDK cannot forcibly stop a native
callback that deliberately blocks forever. Events contain typed IDs, controlled
enums, durations, counts, and booleans—not prompt/output/tool arguments, Header
values, bodies, or secrets. The optional `tracing` feature exports
`TracingObserver`; applications own subscribers, exporters, sampling, retention,
and the rule that high-cardinality IDs never become metrics labels.
