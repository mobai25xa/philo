# Phase-One Fixtures

Fixtures are added with the implementation task that consumes them. This directory starts with the metadata contract so fixtures cannot become unexplained response dumps.

Every fixture entry in [`manifest.toml`](./manifest.toml) must include:

- a stable ID and relative path;
- its purpose and expected result or typed error;
- `synthetic`, `official-doc-example`, or `sanitized-observation` as source;
- source URL and capture/sanitization dates when applicable;
- the behavior contract version;
- notes explaining intentional deviations.

Real observations must remove credentials, endpoints, prompts, outputs, personal information, and all request/generation/trace IDs. A canary scan and human review are required before commit. Replacing text may invalidate observed usage values; mark such usage as synthetic.

Fixture content must be deterministic and tests must not require network access.
