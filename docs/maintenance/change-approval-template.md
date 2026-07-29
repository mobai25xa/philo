# Compatibility change approval

Copy this template into the change description or linked ADR.

## Identity

- change title:
- author:
- target package/version:
- affected stability class:
- linked issue/ADR:

## Classification

- [ ] Rust API
- [ ] Feature/default
- [ ] Behavior/default
- [ ] Config/serde/data
- [ ] Provider drift/support status
- [ ] MSRV/dependency
- [ ] Security exception
- [ ] Documentation only

Proposed SemVer level and policy row:

## Impact inventory

- public paths and feature combinations:
- behavior contracts and defaults:
- readers/writers and stored data:
- providers/models:
- MSRV and dependency graph:
- consumer examples:

## Compatibility evidence

- API diff:
- behavior/fixture diff:
- reader/writer compatibility:
- MSRV build:
- consumer compile:
- provider Canary/support matrix:
- security and redaction tests:

## Migration

- old usage:
- replacement usage:
- behavior difference:
- deprecation `since`:
- removal major or review date:
- changelog/release-note entry:

## Exception record

If a deprecation or MSRV window is shortened:

- threat or operational reason:
- safer replacement:
- user impact and mitigation:
- disclosure timing:
- backport decision:

## Approval

- Core API owner:
- Behavior/Data/Provider owner, as applicable:
- Security owner, if applicable:
- Release owner:
- non-author reviewer:
- decision and date:

No role may be self-filled merely because local tests pass. The release record
links to the actual reviewer identity and exact candidate.

