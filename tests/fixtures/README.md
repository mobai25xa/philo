# Phase Fixtures

Fixtures are added with the implementation task that consumes them. This directory starts with the metadata contract so fixtures cannot become unexplained response dumps.

Phase-two fixtures live under `phase-2/` and cover tools, multimodal requests, structured output, tool streams, history metadata, schema validation, and security canaries. Synthetic P3 thinking-boundary fixtures are tagged `protocol: synthetic-p3-boundary` and are not Official OpenAI conformance.

Every fixture entry in [`manifest.toml`](./manifest.toml) must include:

- a stable ID and relative path;
- its purpose and expected result or typed error;
- `synthetic`, `official-doc-example`, or `sanitized-observation` as source;
- source URL and capture/sanitization dates when applicable;
- the behavior contract version;
- notes explaining intentional deviations.

Real observations must remove credentials, endpoints, prompts, outputs, personal information, and all request/generation/trace IDs. A canary scan and human review are required before commit. Replacing text may invalidate observed usage values; mark such usage as synthetic.

Fixture content must be deterministic and tests must not require network access.

`manifest.toml` is parsed by `fixture_contract.rs`. The test rejects duplicate IDs,
missing or unlisted files, incomplete provenance, missing typed error expectations,
unsafe relative paths, and credential canaries. Invalid UTF-8 is stored as ASCII hex
and CRLF data as escaped bytes so repository-wide LF normalization cannot alter the
fixture; contract tests decode both representations before use.

SSE property tests run 96 cases locally unless `PROPTEST_CASES` is set; CI sets
256. Proptest prints `PROPTEST_RNG_SEED` on failure and accepts the same variable
for deterministic replay. Shrunk failures use Proptest's source-adjacent regression
persistence and may be promoted into this manifest after provenance review.

Phase-one response fixtures live under `responses/openai_chat/`. They are synthetic,
contain no real provider output or identifiers, and cover the frozen text, usage,
finish/DONE, fail-closed choice/tool semantics, JSON error, and truncation matrix.
