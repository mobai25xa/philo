# Security Policy

`philo` is not yet a stable release. Security-sensitive behavior is still treated as a release blocker.

Do not include API keys, prompts, model outputs, complete headers, or unsanitized Provider responses in an issue, test fixture, log, or evidence record. Reports should contain the affected version, error category, redacted reproduction steps, and whether credentials may have been exposed.

The repository does not yet define a private disclosure address. Until one is established, do not publish an active credential or exploit payload; notify the repository owner through the hosting platform's private security-reporting channel.

The phase-one security contract requires a protected header pipeline, bounded cancellation and Drop behavior, default secret redaction, and typed extensions instead of arbitrary request-body overrides.
