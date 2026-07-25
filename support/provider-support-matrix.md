# Phase 3 provider support matrix

> Status: Local Pass / In Review
>
> Machine-readable source: [`provider-support-matrix.toml`](./provider-support-matrix.toml)
>
> Generated as of: 2026-07-25

This checked rendering must not rewrite `Experimental`, `Unknown`, or `Stale`
as `Supported`.

## Status vocabulary

| Status | Meaning |
|---|---|
| Supported | The exact product/model completed current offline and protected online verification |
| Experimental | The integration runs, but its contract or evidence has not reached the stable support gate |
| Unsupported | Official or controlled evidence explicitly proves the case unavailable |
| Unknown | Evidence is insufficient; this is not equivalent to `false` or `Unsupported` |
| Stale | A prior conclusion exists, but its evidence passed `expires_at` |

`OfflineContractVerified` proves the local SDK contract.
`RealProviderVerified` proves protected behavior for an exact provider model.
Hosted protected runs exist for OpenRouter, Z.AI Standard and Z.AI Coding under
candidate `1f54a42176d3f1deaf1eb1bde2681e22a63662b4`; DeepSeek hosted is
deferred. All entries remain `Experimental`.

## Exact product/model matrix

<!-- BEGIN GENERATED SUPPORT MATRIX -->
| Provider | Product | Exact model | Profile | Catalog | Effective | Evidence | Online | Expires |
|---|---|---|---|---|---|---|---|---|
| openrouter | openrouter-chat | nvidia/nemotron-3-ultra-550b-a55b:free | 3.0.0-experimental | Experimental | Experimental | OfflineContractVerified,RealProviderVerified | Pass | 2026-10-23 |
| deepseek | deepseek-chat-openai | deepseek-v4-flash | 3.0.0-experimental | Experimental | Experimental | OfflineContractVerified,RealProviderVerified | Pass | 2026-10-23 |
| zai | zai-standard-api | glm-4.7-flash | 3.0.0-experimental | Experimental | Experimental | OfflineContractVerified,RealProviderVerified | Pass | 2026-10-23 |
| zai | zai-coding-plan | glm-4.7-flash | 3.0.0-experimental | Experimental | Experimental | OfflineContractVerified,RealProviderVerified | Pass | 2026-10-23 |
<!-- END GENERATED SUPPORT MATRIX -->

## Hosted binding

| Provider/Product | Candidate SHA | Run URL |
|---|---|---|
| openrouter/openrouter-chat | `1f54a42176d3f1deaf1eb1bde2681e22a63662b4` | https://github.com/mobai25xa/philo/actions/runs/30106489910 |
| zai/zai-standard-api | `1f54a42176d3f1deaf1eb1bde2681e22a63662b4` | https://github.com/mobai25xa/philo/actions/runs/30106679021 |
| zai/zai-coding-plan | `1f54a42176d3f1deaf1eb1bde2681e22a63662b4` | https://github.com/mobai25xa/philo/actions/runs/30138294979 |
| deepseek/deepseek-chat-openai | empty | Hosted deferred |

## Capability summary

- `text_stream` and `usage_and_request_id` are `Experimental` with offline evidence;
  OpenRouter, Z.AI Standard and Z.AI Coding also have hosted protected online evidence.
- `single_tool` and `thinking_and_replay` remain `Unknown`.
- Real targets currently prove Bearer auth only. API-key Header, multi-header,
  and dynamic-token shapes have offline contract evidence only.
- See [`provider-limitations.md`](./provider-limitations.md) for reachable exact-product limitations.

An entry may become `Supported` only when its exact Catalog key, profile,
compat and contract versions match; required offline cases pass; a protected
hosted online report records the same exact model and candidate SHA; evidence is
current; and independent review has zero blocking findings.
