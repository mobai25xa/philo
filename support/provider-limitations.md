# Phase 3 provider limitations

All entries in this document remain `Experimental` and have
`OfflineContractVerified` evidence only. Protected online runs are pending.

## OpenRouter / `openrouter-chat` / `openai/gpt-4o-mini`

- Typed attribution Headers, routing, endpoint audience, streaming error and
  usage shapes have offline contracts.
- Native cost, provider-specific finish details and reasoning details do not
  enter the public Domain model.
- Single-tool and reasoning replay support remain `Unknown`.

## DeepSeek / `deepseek-chat-openai` / `deepseek-v4-flash`

- Exact endpoint/audience and typed `max_tokens` behavior have offline contracts.
- Thinking replay is not enabled by this profile.
- Strict beta endpoints are excluded from the stable preset boundary.
- Single-tool and reasoning replay support remain `Unknown`.

## Z.AI / `zai-standard-api` and `zai-coding-plan` / `glm-5`

- The two products have distinct path and credential-audience contracts even
  though they share a host and exact model label.
- Standard and coding credentials cannot cross product boundaries.
- Thinking and tool-stream behavior are not enabled.
- Single-tool and reasoning replay support remain `Unknown`.
