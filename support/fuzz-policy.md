# Fuzz and crash policy

The maintained targets are `sse_decoder`, `openai_stream`, `anthropic_stream`,
`endpoint_and_headers`, `domain_schema_history_tools`, `raw_body_and_error`, and
`config_parser`. Pull requests replay their corpus through
`fuzz_regression_contract`; scheduled and release jobs run the mutation engine.

A crash is closed only after minimization and promotion:

```powershell
cargo fuzz tmin <target> <artifact>
pwsh ./tools/promote-fuzz-regression.ps1 -Target <target> -CrashPath <artifact>
cargo test --test fuzz_regression_contract
```

Promoted files are permanent capability-owned regression inputs. They contain
no credentials, prompts, outputs, headers, request IDs, or production traffic.
Corpus growth is reviewed for unique behavior; duplicate byte shapes are not
retained.
