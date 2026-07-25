# Phase 4 migration notes

Phase 4 is additive but changes reliability expectations:

- automatic retry remains off by default; opt in with `RetryPolicy::standard()`;
- any delivered `AssistantEvent`, including tool and usage events, permanently
  closes automatic retry;
- an overall request deadline is shared by every attempt and retry wait;
- request-level timeout/retry/wait policy can only tighten client policy;
- unknown idempotency capability fails closed when a key or replay requirement is
  requested;
- rate metadata is typed and discards raw Header values;
- lifecycle observers receive additional attempt/retry/timeout/rate events and
  their panic no longer changes the main request result;
- unknown external finish labels no longer appear in default `Display`/`Debug`;
- oversized response headers now fail with a value-free protocol error.

No migration should add provider-specific branches to the client, execute tools
inside the SDK, or implement cross-provider fallback as retry.
