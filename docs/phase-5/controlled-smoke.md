# Controlled Anthropic smoke

The live smoke is opt-in and is never part of the default offline test run. Use a
revocable, quota-limited credential and a non-production account. The harness
reads `ANTHROPIC_API_KEY` and `ANTHROPIC_MODEL`; it must not print their values,
prompts, outputs, Tool arguments, thinking, or request IDs.

Run only from a reviewed candidate tree:

```text
cargo test --test anthropic_smoke -- --ignored --exact anthropic_controlled_smoke
```

The operator records only UTC time, candidate commit/tree state, exact model,
API version, beta header names (normally none), region/account class, case result,
and a redacted failure category. The SDK never executes returned Tools and never
downloads image URLs during this smoke. See the stage evidence for the current
execution status.
