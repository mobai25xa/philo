# Phase 4 client soak contract

The soak is offline and uses deterministic mock exchanges. It reuses one client,
drops ten percent of streams before body polling, drains captures periodically,
and requires all scripted exchanges and dropped bodies to be cleaned up.

Profiles:

- `quick`: 1,000 iterations for pull requests and ordinary manual checks;
- `release`: 100,000 iterations for the weekly or manually selected exact SHA;
- `connection-churn`: 25,000 iterations for focused transport work.

On Linux, the harness samples its own resident memory, thread count, open file
descriptors, and socket descriptors. The release profile fails when growth exceeds
the limits in `support/phase4-resource-budgets.toml`. GitHub Actions additionally
requires Linux resource metrics to be present and retains the value-free JSONL
result for 90 days.

The artifact must identify the same commit as the hosted CI run. It contains
counts and resource measurements only; prompts, outputs, credentials, headers,
response bodies, and provider request identifiers are excluded.
