# Candidate public API inventory

This inventory classifies the source tree at commit
`13a6f5c212e469fa06fb571ebd9cb70bc3f2934f`, tree
`a31c80a0e89d9bbf6175b8737420179b977b200d`. It is a decision input, not the
first 1.0 API baseline.

## Classification rule

Every public item inherits the decision of the nearest prefix in the tables
below. Explicit overrides win. A newly reachable item with no matching prefix
is an inventory failure and blocks release. Feature-aware tool output and diff
enforcement verify this reviewed source-prefix inventory.

## Root surface

| Prefix/item | Class | Decision |
|---|---|---|
| `philo::{SDK_NAME, SDK_VERSION}` | Stable | Keep |
| `philo::{OPENAI_CHAT_CONTRACT_ID, OPENAI_CHAT_CONTRACT_VERSION}` | Stable | Keep |
| `philo::{RELIABILITY_CONTRACT_ID, RELIABILITY_CONTRACT_VERSION}` | Stable | Keep |
| `philo::{PROVIDER_CONFIG_SCHEMA_ID, PROVIDER_CONFIG_SCHEMA_VERSION}` | Experimental | Keep with the serialization inventory as data owner |
| documented first-request re-exports from `client`, `domain`, `error`, reliability, provider, and transport | Stable unless overridden below | Keep |
| stage-numbered public contract constants | Removed before baseline | Removed |

## Core module prefixes

| Prefix | Class | Enum/extension strategy | Decision |
|---|---|---|---|
| `philo::client::*` | Stable | public result/event additions follow SemVer | Keep |
| `philo::domain::ids::*` | Stable | opaque validated identifiers; representation is not public | Keep |
| `philo::domain::message::*` | Stable | closed role/message enums; a new variant is breaking | Keep |
| `philo::domain::event::*` | Stable | event ordering is behavior-compatible surface; new event variants require non-exhaustive review | Keep; closed enums are intentional |
| `philo::domain::request::*` | Stable except thinking/reasoning overrides | closed enums are intentionally breaking to extend unless made `non_exhaustive` before baseline | Keep |
| `philo::domain::content::{ThinkingContent, OpaqueReasoning}` | Experimental | migration note required for Minor changes | Keep Experimental |
| `philo::domain::request::{ThinkingRequest, ReasoningEffort, ReasoningEffortSupport}` | Experimental | migration note required for Minor changes | Keep Experimental |
| `philo::domain::structured::*` | Experimental | schema safety remains Stable | Keep Experimental |
| `philo::domain::tools::*` | Stable | application execution is explicitly out of scope | Keep |
| `philo::domain::history::*` | Stable, with thinking wire/replay policy Experimental | policy enums are closed unless marked `non_exhaustive`; additions otherwise break | Keep |
| `philo::domain::usage::*` | Stable | absence/zero distinction and merge rules are behavior contract | Keep |
| `philo::error::*` | Stable | `LlmError` and most category enums are `non_exhaustive`; category changes are breaking | Keep |
| `philo::transport::*` | Stable | network transport and framing contracts | Keep |
| `philo::observability::*` | Stable value-free lifecycle contract | `TracingObserver` is Experimental | Keep |
| `philo::protected::*` | Stable safety contract | protected tables may grow in a Patch security release | Keep |
| `philo::protocol_options::{ProtocolOptions, OpenAiChatOptions}` | Stable | enum is `non_exhaustive` | Keep |
| `philo::protocol_options::{AnthropicMessagesOptions, AnthropicEffort, AnthropicThinkingDisplay}` | Experimental | Minor changes require migration note | Keep Experimental until Anthropic promotion |
| `philo::protocol_options::{OpenAiChatRawExtension, AnthropicRawExtension}` | Escape Hatch | safety/resource API Stable; raw semantics non-portable | Keep |

## Provider module prefixes

| Prefix | Class | Decision |
|---|---|---|
| `philo::provider::{definition, endpoint, auth, headers, secret, runtime}` and their re-exports | Stable | Keep fail-closed construction and credential boundaries |
| `philo::provider::{catalog, capability}` | Stable for types and Unknown/Unsupported/Supported semantics | Keep; provider values/freshness remain external data |
| `philo::provider::{registry, factory}` | Stable | Keep explicit selection; no provider-name protocol guessing |
| `philo::provider::{rate_limit, idempotency}` | Stable metadata | Keep; no automatic billing or cross-provider policy |
| `philo::provider::profiles::OfficialOpenAiProfile` | Stable | Keep |
| `philo::provider::profiles::OfficialAnthropicProfile` and `OFFICIAL_ANTHROPIC_API_VERSION` | Experimental | Promote only with same-candidate controlled smoke |
| `philo::provider::protocol_contract::*` | Stable resolved contract | Keep; sparse merge policy does not belong in core |

## Companion packages

| Prefix | Class | Decision |
|---|---|---|
| `philo_config::*` | Experimental | Keep separate under its schema compatibility owner |
| `philo_presets::*` | Experimental | Keep separate only with provider owner, Canary, and expiry |

## Features and hidden public items

- `rustls-tls`: Stable and default. Default-feature changes are breaking.
- `tracing`: Experimental opt-in adapter. Lifecycle event/redaction contracts
  remain Stable.
- `#[doc(hidden)] pub` is still public for SemVer purposes and must be included
  in the API diff.

## Behavior, data, and environment surface

| Surface | Class | Owner/decision |
|---|---|---|
| timeout, retry, redirect, cancellation, drop, and retry-after behavior | Stable | Reliability; defaults are SemVer surface |
| header/auth precedence, protected names, exact credential origin | Stable | Provider Security |
| event order, finish/error classification, usage merge | Stable | Domain/Protocol |
| Debug/Display/redaction and body bounds | Stable security behavior | Security |
| official credential variables used only by opt-in smoke workflows | Internal operations | Never a runtime library configuration API |
| `SecretReference` serde representation | Candidate Stable data | reader/writer behavior is owned by the serialization inventory |
| `philo-config` serde schema and unknown-field behavior | Experimental data | Configuration owner |
| protocol wire JSON and SSE fixture representation | Internal protocol evidence | Not a user persistence format |
| `AssistantMessage` in-memory shape | Stable Rust API | No Stable serde persistence promise is made |

## Baseline blockers and owners

1. API Compatibility: audit public closed enums, trait bounds, and auto traits
   with tool-produced metadata; add `#[non_exhaustive]` only where extension is
   intended.
2. Provider Compatibility: obtain Official Anthropic same-candidate controlled
   smoke before promotion to Stable.
3. Data Compatibility: preserve the reviewed serde classification and current
   fixture contract IDs.

No 1.0 baseline or stable tag may be created while item 1 remains open.
