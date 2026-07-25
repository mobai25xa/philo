# Phase 3 provider limitations

All entries in this document remain `Experimental`. Offline contract evidence
exists for all four products. Hosted protected online evidence covers OpenRouter,
Z.AI Standard and Z.AI Coding under candidate
`1f54a42176d3f1deaf1eb1bde2681e22a63662b4`. DeepSeek hosted is deferred.
Support does not upgrade to `Supported` until full gate and independent review
conditions are met.

## OpenRouter / `openrouter-chat` / `nvidia/nemotron-3-ultra-550b-a55b:free`

- Hosted protected smoke passed: https://github.com/mobai25xa/philo/actions/runs/30106489910
- Typed attribution Headers, routing, endpoint audience, streaming error and
  usage shapes have offline contracts.
- Native cost, provider-specific finish details and reasoning details do not
  enter the public Domain model.
- Single-tool and reasoning replay support remain `Unknown`.

## DeepSeek / `deepseek-chat-openai` / `deepseek-v4-flash`

- Local online text/usage cases passed; Hosted protected run is deferred.
- Exact endpoint/audience and typed `max_tokens` behavior have offline contracts.
- Thinking replay is not enabled by this profile.
- Strict beta endpoints are excluded from the stable preset boundary.
- Single-tool and reasoning replay support remain `Unknown`.

## Z.AI Standard / `zai-standard-api` / `glm-4.7-flash`

- Hosted protected smoke passed: https://github.com/mobai25xa/philo/actions/runs/30106679021
- The Standard API credential is restricted to the exact PaaS endpoint audience.
- Thinking and tool-stream behavior are not enabled.
- Single-tool and reasoning replay support remain `Unknown`.

## Z.AI Coding / `zai-coding-plan` / `glm-4.7-flash`

- Hosted protected smoke passed: https://github.com/mobai25xa/philo/actions/runs/30138294979
- The Coding Plan has a distinct path and credential-audience contract even
  though it shares a host with the Standard API.
- Standard and Coding credentials cannot cross product boundaries.
- Thinking and tool-stream behavior are not enabled.
- Single-tool and reasoning replay support remain `Unknown`.
