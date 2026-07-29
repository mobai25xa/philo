# Security Policy

`philo` is not yet a stable release. Security-sensitive behavior is a release blocker.

Do not include API keys, prompts, model outputs, complete headers, or unsanitized Provider responses in an issue, test fixture, log, or evidence record. Reports should contain the affected version, error category, redacted reproduction steps, and whether credentials may have been exposed.

The repository does not yet define a private disclosure address. Until one is established, do not publish an active credential or exploit payload; notify the repository owner through the hosting platform's private security-reporting channel.

The security contract requires a protected header pipeline, bounded cancellation and Drop behavior, default secret redaction, and typed extensions instead of arbitrary request-body overrides.

## Credential and Endpoint Policy

- Production examples construct only `OfficialOpenAiProfile` and accept API keys only
  from the caller or `OPENAI_API_KEY`; the SDK does not read developer config files.
- Official credentials are audience-bound to `https://api.openai.com` and cannot be
  attached to test or arbitrary compatible endpoints.
- The localhost profile is hidden, test-only, restricted to loopback, and absent from
  normal examples.
- Redirects are disabled by default. Cross-origin redirects never receive the
  original Authorization header.

## Header, Body, and Diagnostic Policy

Ordinary overrides cannot set or remove Authorization, Host, Content-Length,
Content-Type, or connection-specific headers. Header values reject HTTP CR/LF
injection through typed `http` values and final pipeline validation.

Lifecycle diagnostics contain typed IDs, elapsed durations, status, stable error
categories, and header names/sources/decisions only. They do not contain header
values, prompt, model output, request/response bodies, or secret fingerprints. HTTP
error bodies are read to a fixed limit and converted to a redacted summary.

## Smoke Policy

Online Provider verification is opt-in through the single
`.github/workflows/canary.yml` entry. It runs only when the exact commit, target,
model, and reviewed case are explicit and the selected credential is supplied by
the protected `provider-canary` environment. Secret-bearing code must never run
from an unreviewed commit. The sole Canary result format never includes prompts,
outputs, credentials, headers, bodies, Tool arguments, thinking, image URLs, or
ProviderRequestId/GenerationId values.
