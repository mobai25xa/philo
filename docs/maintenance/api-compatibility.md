# API and behavior compatibility gate

`tools/check-api-compatibility.ps1` compares the `philo` package with the newest
`philo-v*` stable tag using `cargo-semver-checks`. It runs the default,
no-default-feature, and all-feature surfaces and writes JSON plus per-surface
logs under `target/compatibility/api/`.

Until the required API and Release external sign-offs exist, the repository has
no legitimate Stable tag. CI therefore uses `-AllowBootstrap` and emits an explicit
`bootstrap-pending` report instead of treating the current alpha checkout as a
baseline. Release-candidate execution omits that switch and fails if no Stable
tag exists.

Breaking results fail by default. A planned new-major change additionally needs
a reviewed record under `compatibility/approvals/`, an ADR, a migration guide,
and distinct API/Release reviewers. The raw diff logs and approval path remain in
the report artifact.

Stable and Experimental consumers are separate workspaces under `consumers/`.
CI compiles Stable consumers on the declared MSRV and current stable toolchain,
including no-default/all-feature combinations. Experimental configuration and
provider presets cannot silently expand the core 1.0 promise.

Behavior ownership lives in `support/behavior-contracts.toml`. Golden changes
are classified as `Compatible`, `Bug Fix`, `Breaking`, or `External Drift` using
`docs/maintenance/change-approval-template.md`; snapshots are never auto-accepted.
