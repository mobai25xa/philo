# Capability-owned fixtures

Fixtures are organized by their long-lived owner: `domain/`, `protocol/`,
`provider/`, or `transport/`. A fixture exists only when it reproduces a public
behavior, protocol boundary, security invariant, or provider compatibility case.

Every fixture entry in [`manifest.toml`](./manifest.toml) must include:

- a stable ID and relative path;
- its purpose and expected result or typed error;
- `synthetic`, `official-doc-example`, or `sanitized-observation` as source;
- category, format version, protocol, and provider/product when applicable;
- source URL, review date, redaction status, license/permission, and public allowance;
- the explicit behavior contract ID and version;
- notes explaining intentional deviations.

Real observations must remove credentials, endpoints, prompts, outputs, personal information, and all request/generation/trace IDs. A canary scan and human review are required before commit. Replacing text may invalidate observed usage values; mark such usage as synthetic.

Fixture content must be deterministic and tests must not require network access.

`manifest.toml` is parsed by `fixture_contract.rs`. The test rejects duplicate IDs,
missing or unlisted files, incomplete provenance, missing typed error expectations,
unsafe relative paths, and credential canaries. Invalid UTF-8 is stored as ASCII hex
and CRLF data as escaped bytes so repository-wide LF normalization cannot alter the
fixture; contract tests decode both representations before use.

Manifest schema v3 does not infer metadata from a directory name. IDs and paths
are semantic and contain no development-stage number.

SSE property tests run 96 cases locally unless `PROPTEST_CASES` is set; CI sets
256. Proptest prints `PROPTEST_RNG_SEED` on failure and accepts the same variable
for deterministic replay. Shrunk failures use Proptest's source-adjacent regression
persistence and may be promoted into this manifest after provenance review.

OpenAI response fixtures live under `protocol/openai_chat/stream/`. They are
synthetic, contain no real provider output or identifiers, and cover text,
usage, finish/DONE, fail-closed choice/tool semantics, JSON error, and truncation.
