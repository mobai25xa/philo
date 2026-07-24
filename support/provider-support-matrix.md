# Phase 3 provider support matrix

> Status: Local Pass / In Review
>
> Machine-readable source: [`provider-support-matrix.toml`](./provider-support-matrix.toml)
>
> Generated as of: 2026-07-24

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
The four third-party products below currently have only offline evidence.

## Exact product/model matrix

<!-- BEGIN GENERATED SUPPORT MATRIX -->
| Provider | Product | Exact model | Profile | Catalog | Effective | Evidence | Online | Expires |
|---|---|---|---|---|---|---|---|---|
| openrouter | openrouter-chat | nvidia/nemotron-3-ultra-550b-a55b:free | 3.0.0-experimental | Experimental | Experimental | OfflineContractVerified | Pending | 2026-10-23 |
| deepseek | deepseek-chat-openai | deepseek-v4-flash | 3.0.0-experimental | Experimental | Experimental | OfflineContractVerified | Pending | 2026-10-23 |
| zai | zai-standard-api | glm-4.7-flash | 3.0.0-experimental | Experimental | Experimental | OfflineContractVerified | Pending | 2026-10-23 |
| zai | zai-coding-plan | glm-5 | 3.0.0-experimental | Experimental | Experimental | OfflineContractVerified | Pending | 2026-10-23 |
<!-- END GENERATED SUPPORT MATRIX -->

## Capability summary

- `text_stream` and `usage_and_request_id` are `Experimental` with offline evidence.
- `single_tool` and `thinking_and_replay` remain `Unknown`.
- Real targets currently prove Bearer auth only. API-key Header, multi-header,
  and dynamic-token shapes have offline contract evidence only.
- See [`provider-limitations.md`](./provider-limitations.md) for reachable exact-product limitations.

An entry may become `Supported` only when its exact Catalog key, profile,
compat and contract versions match; required offline cases pass; a protected
online report records the same exact model and candidate SHA; evidence is
current; and independent review has zero blocking findings.
