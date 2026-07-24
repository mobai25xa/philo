# Phase 3 provider limitations

All entries in this document remain `Experimental`. Offline contract evidence and
local worktree online evidence exist; Hosted CI exact-SHA protected runs are
still pending, so support does not upgrade to `Supported`.

## OpenRouter / `openrouter-chat` / `nvidia/nemotron-3-ultra-550b-a55b:free`

- Local online text/usage cases passed for this exact free model.
- Typed attribution Headers, routing, endpoint audience, streaming error and
  usage shapes have offline contracts.
- One payload-free duplicate of the same normalized finish reason is treated as
  an idempotent gateway terminal replay; conflicts, late deltas and additional
  repeats remain protocol errors.
- Native cost, provider-specific finish details and reasoning details do not
  enter the public Domain model.
- Single-tool and reasoning replay support remain `Unknown`.

## DeepSeek / `deepseek-chat-openai` / `deepseek-v4-flash`

- Local online text/usage cases passed for this exact model.
- Exact endpoint/audience and typed `max_tokens` behavior have offline contracts.
- Thinking replay is not enabled by this profile.
- Strict beta endpoints are excluded from the stable preset boundary.
- Single-tool and reasoning replay support remain `Unknown`.

## Z.AI Standard / `zai-standard-api` / `glm-4.7-flash`

- Local online text/usage cases passed for this exact model.
- The Standard API credential is restricted to the exact PaaS endpoint audience.
- Thinking and tool-stream behavior are not enabled.
- Single-tool and reasoning replay support remain `Unknown`.

## Z.AI Coding / `zai-coding-plan` / `glm-4.7-flash`

- Local online text/usage cases passed for this exact model on the Coding path.
- The Coding Plan has a distinct path and credential-audience contract even
  though it shares a host with the Standard API.
- Standard and Coding credentials cannot cross product boundaries.
- Thinking and tool-stream behavior are not enabled.
- Single-tool and reasoning replay support remain `Unknown`.
