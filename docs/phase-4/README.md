# Phase 4 release evidence

Contract: `philo/reliability-p4` version `1.0.0`.

This directory contains the rules and evidence format for binding the Phase 4
release Gate to one exact Git commit. The commit SHA is recorded at execution time
in the local Gate output, GitHub Actions job summary, and retained soak artifact;
it is not embedded into its own commit.

Release evidence consists of:

- a clean exact-SHA local Gate run;
- the multi-platform, MSRV, feature, dependency, and quality jobs in `ci.yml`;
- the exact-SHA benchmark/fault/soak job in `phase4-reliability.yml`;
- the value-free `phase4-soak-result.jsonl` artifact;
- a separate independent review decision.

See [release gate](./release-gate.md) and [soak contract](./soak.md).
