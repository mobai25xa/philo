# Compatibility Policy

This policy defines the compatibility contract for `philo` and its companion
packages. It applies to every release, including the pre-1.0 series.

## Stability classes

Every public capability and API path inherits one of these classes from
[`docs/maintenance/capability-inventory.toml`](./docs/maintenance/capability-inventory.toml)
and [`docs/maintenance/public-api-inventory.md`](./docs/maintenance/public-api-inventory.md).

| Class | Promise |
|---|---|
| Stable | Rust API, documented behavior, defaults, and stable data formats follow SemVer. |
| Experimental | May change in a Minor release. Every change still needs a changelog entry and migration note. |
| Escape Hatch | Safety, validation, redaction, and resource limits are Stable; provider portability and raw semantics are not. |
| Internal | No external compatibility promise. It must not be reachable from a published crate. |
| Removed before baseline | Alpha-only surface that must be removed before the first 1.0 API baseline. |
| Deferred 1.x | Not part of 1.0. It may later be added compatibly. |

An item inherits the class of its nearest listed module or prefix unless the
inventory names an override. Unlisted public items are a release blocker; they
do not silently inherit Stable status.

## Version and package policy

- `philo` is the only package selected for a 1.0 Stable release.
- `philo-config` and `philo-presets` remain independently versioned 0.x
  Experimental companion packages. Publishing either package requires its own
  Beta/RC evidence; publishing core does not imply that they are supported.
- Pre-release versions use `MAJOR.MINOR.PATCH-beta.N` and
  `MAJOR.MINOR.PATCH-rc.N`. A pre-release dependency on another workspace
  package is exact.
- Stable companion packages use the Cargo-compatible range for the supported
  core major, for example `philo = "1"`; a tighter bound is allowed when a
  capability requires it.
- Tags are package-qualified: `philo-v1.0.0`, `philo-config-v0.2.0`, and
  `philo-presets-v0.2.0`. Cargo metadata, tag, release notes, and documentation
  version must agree.
- Release order follows dependency order: core, then config, then presets.
  A partial failure never reuses a version. Already published packages remain
  published; the failed package is corrected and released under a new version.

## SemVer decision matrix

The highest-impact applicable row wins.

| Change | Stable | Experimental | Escape Hatch |
|---|---|---|---|
| Add a Builder method or optional argument with no default change | Minor | Minor | Minor |
| Remove, rename, restrict visibility, or change a public signature | Major after deprecation | Minor with migration note | Major for the safety API; Minor for raw semantics |
| Add a variant to a closed public enum | Major | Minor with migration note | Major when callers match the safety result |
| Add a variant to `#[non_exhaustive]` enum | Minor | Minor | Minor |
| Add a required trait method or stronger bound | Major | Minor with migration note | Major for a Stable trait |
| Remove `Send`, `Sync`, or another documented auto trait | Major | Minor with migration note | Major for the bounded wrapper |
| Change default timeout, retry, redirect, header, TLS, or limit behavior | Major unless strictly safer and approved | Minor with migration note | Major for a documented safety boundary |
| Add a new typed error while preserving the caller's existing category | Minor | Minor | Minor |
| Reclassify an existing error or change retry eligibility | Major | Minor with migration note | Major when it changes fail-open/fail-closed behavior |
| Change `Display`, `Debug`, `source`, or redaction | Patch only when diagnostics stay value-free; otherwise Major | Minor | Security exception may force Patch |
| Add a default feature or remove/change a default feature | Major | Minor with migration note | Major if security or dependency behavior changes |
| Add an opt-in feature | Minor | Minor | Minor |
| Change Stable config/serde field names, tags, defaults, or unknown-field behavior | Major unless a reader/writer migration proves compatibility | Minor with migration note | Not applicable unless explicitly listed |
| Remove or downgrade a provider preset | Minor with provider notice if external drift caused it; otherwise Major | Minor | Minor |
| Raise MSRV | Minor under the MSRV policy | Minor | Minor |
| Tighten a security boundary | Patch under the security exception process | Patch or Minor | Patch |

Bug fixes are Patch releases only when they restore documented behavior. If a
widely used observable behavior contradicts the documentation, maintainers must
perform an impact review instead of automatically calling the change a fix.

## Behavior compatibility

Stable behavior includes:

- request and stream event ordering;
- error category and retry decision;
- timeout, cancellation, redirect, and retry boundaries;
- header/auth precedence and protected fields;
- fail-closed capability checks;
- secret and content redaction;
- bounded body, SSE, raw extension, and history limits;
- documented defaults and environment-variable meaning.

Golden or fixture updates require a behavior classification. Updating expected
output is not approval by itself.

Provider-side drift is not automatically an SDK breaking change. The response
is to update Canary evidence, support status, compatibility notes, and offline
fixtures. An SDK API or default changed in response to drift still follows this
policy.

## MSRV policy

- The 1.0 candidate MSRV is Rust `1.97.1`, shared by every publishable workspace
  package.
- CI tests the exact MSRV declared in `package.rust-version`; `stable` is a
  separate job and does not substitute for MSRV.
- The project supports at least the six most recent Rust minor releases and at
  least six months from an MSRV announcement, whichever window is longer.
- MSRV may be raised in a Minor release, no more than once per three months, with
  at least 30 days' notice in `CHANGELOG.md`.
- Dependency updates must pass the exact-MSRV build. A dependency that silently
  raises MSRV is pinned, downgraded, replaced, or accompanied by an approved MSRV
  change.
- Nightly is advisory only. It may detect future warnings but is not a supported
  compiler contract.
- A critical security fix may raise MSRV sooner only with Security and Release
  owner approval, impact notes, and a best-effort backport to the prior MSRV.

## Deprecation policy

A Stable item is normally deprecated for at least two Minor releases and six
months, whichever is longer. Every deprecation must state:

- `since` version;
- replacement path;
- behavior differences;
- planned removal major or review date;
- a compiling migration example;
- any security exception.

Alpha-only items classified `Removed before baseline` are removed before 1.0
without a Stable deprecation cycle, but the removal is recorded in the
changelog and migration notes.

A critical vulnerability, credential leak, unsafe redirect, unbounded resource
path, or provider behavior that can no longer be implemented safely may shorten
the cycle. Security and Release owners must approve the exception, publish the
reason without disclosing exploit details prematurely, and provide the safest
available replacement.

## Change approval

All compatibility-sensitive changes use
[`docs/maintenance/change-approval-template.md`](./docs/maintenance/change-approval-template.md).

- Breaking changes are blocked by default and require an ADR, impact inventory,
  migration, target version, API/behavior/data diff, and Core API approval.
- Behavior changes require the behavior owner and a non-author reviewer.
- Stable data changes require reader/writer compatibility evidence and the Data
  owner.
- Security exceptions require both Security and Release owner approval.
- Provider drift changes require the Provider owner and updated support status.
- Experimental changes still require a changelog entry and migration note.

Role names are durable ownership boundaries, not claims that a particular
person has already signed a release. Actual sign-off is recorded on the change
or release candidate.

## Hotfix, backport, and yank

- Supported stable lines receive security and high-severity correctness fixes.
  Normal development targets the latest Minor line.
- A hotfix is a Patch from the affected release tag and contains no unrelated
  refactor or feature.
- A backport must preserve the target line's MSRV and public contract. If that is
  impossible, release notes explain the limitation and safest mitigation.
- Yank is reserved for unusable, compromised, or dangerously misleading
  releases. It does not replace a fixed release or security advisory.
- Published crate contents are immutable. Never recreate or overwrite a version.

## Classification examples

- Adding `Builder::with_optional_limit` without changing defaults: Minor.
- Adding a variant to a closed Stable enum: Major; adding it to a documented
  `#[non_exhaustive]` enum: Minor.
- Changing the default overall timeout: Major unless an approved security
  exception applies.
- Removing an Experimental third-party preset after provider drift: Minor with
  migration and support-matrix updates.
- Raising MSRV from 1.97.1: Minor after the notice and support window.
- Preventing a credential from crossing an origin boundary: Patch security fix,
  with Security and Release approval.
