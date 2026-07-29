# Maintenance contracts

These files are the inputs to the 1.0 compatibility and release gates:

- `api-compatibility.md` defines API baseline bootstrap, feature-aware diff,
  consumer compilation, breaking approvals, and behavior review;
- `serialization-inventory.toml` classifies every production serde type as a
  persistence, diagnostic, wire, internal-state, or test-fixture format;

- [`capability-inventory.toml`](./capability-inventory.toml): machine-readable
  capability, package, and feature decisions;
- [`public-api-inventory.md`](./public-api-inventory.md): public-path and behavior
  classification rules;
- [`change-approval-template.md`](./change-approval-template.md): required review
  record for compatibility-sensitive changes.

The public policy is [`../../COMPATIBILITY.md`](../../COMPATIBILITY.md). These
files describe the candidate surface; they are not an API baseline tag.

## Release owner

`.github/workflows/release.yml` is the only release entry. The Release owner
supplies one candidate SHA plus successful CI, scheduled fuzz, Linux release
soak, official OpenAI Canary, and official Anthropic Canary run IDs. The
workflow rejects any run whose `head_sha` differs, builds the source package,
SBOM, release notes, and sole release manifest, and performs an external
consumer dry-run without writing the registry, tag, or release.

`mode=publish` adds the protected `stable-release` environment after the dry-run
job. It publishes only `philo`, verifies the registry checksum and a fresh
registry consumer, then creates `philo-v<version>` and the matching release.
Failures stop before later writes; an already published version follows the
hotfix/yank policy in `COMPATIBILITY.md` and is never overwritten.

Provider drift is owned by the single protected `.github/workflows/canary.yml`
entry and `support/provider-support-matrix.toml`. Private vulnerability reports
use the hosting platform channel defined in `SECURITY.md`.
