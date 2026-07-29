# Changelog

All notable changes are recorded here. The format follows Keep a Changelog and
the project follows the rules in [`COMPATIBILITY.md`](./COMPATIBILITY.md).

## Unreleased

## 1.0.0-rc.1

### Changed

- Classified the candidate 1.0 capability, package, feature, API, behavior, and
  repository boundaries.
- Replaced public phase-numbered behavior metadata with capability-based
  `OPENAI_CHAT_*` and `RELIABILITY_*` constants.
- Published the SemVer, MSRV, deprecation, change-approval, hotfix, backport, and
  yank policies.

### Removed

- Removed the unconsumed `philo-compat` package.
- Removed `philo-test-support`, the `test-util` public feature, and public mock
  transport/profile helpers; repository tests now own their private support.

## 0.1.0-alpha.1

Initial alpha implementation. Construction history is not a Stable
compatibility promise.
