# Provider Compatibility Fixture Provenance

This directory owns provider compatibility fixtures. Implemented Experimental profiles use synthetic offline fixtures for OpenRouter, DeepSeek, Z.AI standard, and Z.AI coding. These fixtures validate local contracts; they are not controlled online captures and do not by themselves establish provider conformance.

Every fixture file must be accompanied by a manifest entry containing all fields below:

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

Current provider directories contain synthetic contract, SSE, error, and wire fixtures registered in `tests/fixtures/manifest.toml`. Protected online smoke reports remain external evidence and must not be represented by editing these synthetic files.
