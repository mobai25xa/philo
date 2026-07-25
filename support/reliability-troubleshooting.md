# Reliability troubleshooting

Inspect typed errors and lifecycle fields only. Never troubleshoot by printing a
prompt, output, tool arguments, API key, proxy credential, complete HeaderMap,
HTTP body, or URL query.

| Observed error/event | Safe checks and likely causes | Caller action | Unsafe action |
|---|---|---|---|
| 401/403 or `Authentication` | status, provider, credential source kind, attempt number; expired/wrong-audience credential | verify audience and dynamic refresh policy; rotate outside logs | print Authorization or token source output |
| 429 / `RateLimitObserved` | `retry_after_valid`, typed remaining/reset state, attempt and deadline budget | opt into bounded retry or schedule later | copy raw rate headers into logs/metrics labels |
| Connect timeout/failure | `TimeoutStage::Connect`, network policy, endpoint origin, proxy mode | verify DNS/proxy/CA and loopback reproduction | disable TLS/SSRF checks |
| Response-header timeout | `ResponseHeader`, attempt elapsed, overall remaining | increase only the justified stage/overall budget | reset the deadline on every attempt |
| First-event timeout | `FirstEvent`, no public delivery, retry decision | retry only within budget; inspect provider load | treat arbitrary bytes as domain progress |
| Idle timeout | `IdleStream`, `partial_output=true` | preserve delivered output and surface terminal failure | retry and replay delivered events |
| Overall timeout | `overall_limited=true`, one logical deadline | reduce waits/attempts or set a larger caller deadline | create an unbounded request |
| `RetryExhausted` | `RetryStopReason`, attempt count, remaining deadline | handle final typed error; choose an explicit policy next call | hidden infinite retries |
| Truncated stream | whether any event crossed delivery boundary | retry only when nothing was delivered and replay is safe | concatenate a fresh generation to partial output |
| Cancel/drop | `RequestCancelled`, `partial_output`, body cleanup counters in tests | treat as terminal; create a new logical request if desired | reuse cancelled stream state |
| TLS/proxy/DNS/redirect | controlled network error stage and policy | validate origin, CA, proxy/no-proxy, redirect mode | forward provider auth cross-origin |
| Resource limit/backpressure | protocol/resource message and configured ceiling | reject or deliberately revise reviewed limits | remove limits or buffer the whole stream |
| Idempotency Unsupported/Unknown | capability and key presence/source, never key value | omit key or use a reviewed supported provider policy | claim exactly-once behavior |
| Observer/tracing issue | observer enabled, controlled event kind, subscriber config | make callback non-blocking; test without observer | let hooks mutate retry/route/header decisions |

If application logs already contain a secret, stop further emission, rotate the
secret, follow the application's retention/deletion process, and inspect its
formatter/exporter. The SDK can keep its own errors/events value-free; it cannot
erase downstream logs, crash dumps, process memory, or third-party telemetry.

Verification commands are listed in the Phase 4 evidence README. Reproduce with
MockTransport or loopback first; ordinary CI must remain offline and must not call
a paid provider.
