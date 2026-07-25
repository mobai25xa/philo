# Phase 4 release gate

The candidate is one full 40-character Git commit SHA. Any change to source,
tests, fixtures, benchmarks, workflow, resource budgets, or public documentation
creates a new candidate and invalidates earlier results.

Run the local Gate from the repository root after committing the candidate:

```powershell
$candidate = (git rev-parse HEAD).Trim()
pwsh -File tools/run-phase4-gate.ps1 `
  -ExpectedCandidate $candidate `
  -RequireCleanCandidate `
  -IncludeBenchmark
```

Push that commit and require all jobs from `ci.yml` plus the
`phase4-reliability.yml` job to pass against the same SHA. For a manual release
soak, dispatch `Phase 4 reliability` with the full candidate SHA and the `release`
profile.

The Gate is Ready only when local, hosted CI, release soak, and independent review
all pass without unresolved blocking findings.
