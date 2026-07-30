# Breaking API approvals

Compatibility failures are blocked by default. A planned breaking change for a
new major may pass `tools/check-api-compatibility.ps1 -ApprovalFile <path>` only
with a reviewed TOML record in this directory.

Required fields:

```toml
schema_version = 1
kind = "api"
status = "approved"
baseline_ref = "philo-v1.2.3"
target_version = "2.0.0"
adr = "docs/maintenance/adr/0001-example.md"
migration = "docs/migration/2.0.md"
api_reviewer = "github-login-a"
release_reviewer = "github-login-b"
```

The ADR and migration paths must exist, and the two reviewer identities must be
distinct. Approval records are release evidence; they are never generated or
auto-accepted by the diff tool.
