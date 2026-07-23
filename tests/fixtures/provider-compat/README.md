# Provider Compatibility Fixture Provenance

This directory is the Phase 3 intake contract for provider compatibility fixtures. P2.5 uses only synthetic, in-process contract cases; it does not claim conformance for OpenRouter, DeepSeek, Z.AI, or any other provider that has no implemented profile.

Every future fixture file must be accompanied by a manifest entry containing all fields below:

```text
id:
provider:
protocol:
model:
capability:
source: official-doc | controlled-capture | synthetic
source_url_or_record:
captured_or_reviewed_at:
contract_version:
expected: success | error
expected_error:
redaction_review:
notes:
```

Provenance rules:

- `official-doc` must identify the exact official source and review date.
- `controlled-capture` must be redacted, reproducible, and reference the controlled record rather than a live credential.
- `synthetic` must be labelled synthetic and must never be presented as provider conformance.
- Every fixture must be registered in `tests/fixtures/manifest.toml` and committed with its consuming test.
- Never store API keys, Authorization values, complete prompts/outputs, tool arguments, image URLs or payloads, or provider request IDs.
- A provider field change requires a contract review and a new `contract_version` decision before updating expected output.

P2.5 intentionally contains no fixture files in this directory. The README is the reusable intake specification, not a conformance result.
