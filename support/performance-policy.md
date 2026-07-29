# Performance and resource policy

`performance-budgets.toml` is the only machine-readable source for metrics,
iterations, repetitions, environment identity, warning/blocking thresholds, and
absolute resource ceilings. Harnesses must read it at runtime and must not copy
release thresholds into Rust or workflow files.

## Comparable baseline

A baseline is five full `sdk_bench` repetitions from one clean exact SHA on the
declared runner. For every metric, the median is the comparison value. A report
is comparable only when schema version, OS, architecture, Rust version, release
profile, and feature set all match. Baselines expire after 30 days or whenever
the runner image, toolchain, feature set, harness, fixture, or budget schema
changes.

The harness reads an approved JSONL baseline from
`PHILO_PERFORMANCE_BASELINE`. A regression at or above the warning threshold is
reported for review; a regression at or above the blocking threshold fails.
The manual workflow resolves that file from an explicit `baseline_run_id`, and
a `release` soak fails before execution when no approved run is supplied.
One noisy sample never becomes a baseline. Suspected noise is resolved by a new
five-repetition run on the same runner, not by rerunning until one value passes.

The percentages in `performance-budgets.toml` are explicitly provisional until
the first repeated hosted run is reviewed. That approval must also remove the
provisional status; the current values are plumbing defaults, not a claimed 1.0
performance baseline. Baseline comparison is disabled unless `status` is changed
to `approved` in the same reviewed budget-only decision.

An intentional regression requires a separate budget change, user-impact
reason, before/after distributions, Performance owner approval, and Release
owner approval. Updating a baseline in the same change as an unexplained
regression is prohibited.

## Resource soak

Quick soak is deterministic and cross-platform. Linux release soak additionally
enforces RSS, thread, file descriptor, and socket deltas. Other platforms report
`resource_metrics_available=false`; that is an explicit capability limitation,
not a zero measurement. Release evidence requires Linux metrics.

The report is value-free. It may contain metric names, counts, durations,
resource deltas, schema/environment identity, and candidate SHA. It must not
contain prompts, outputs, credentials, header values, request IDs, or bodies.

## Scheduling

- Pull request: deterministic reliability/fault tests, benchmark compile, smoke,
  and quick soak.
- Scheduled: five comparable benchmark repetitions and release soak.
- Release candidate: the same scheduled profile bound to the exact candidate
  SHA, with an approved baseline supplied for comparison.

The uploaded `performance-<sha>-<profile>` artifact is the summary consumed by
the release pipeline. It is evidence only for the SHA and environment recorded
inside each JSON line.
