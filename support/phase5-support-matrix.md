# Phase 5 protocol support matrix

Generated as of 2026-07-26. `Experimental` means offline contracts pass but the
same-candidate controlled provider smoke and independent release review are not
both complete. `Unknown` fails closed and is not support.

| Capability | OpenAI Chat Completions | Anthropic Messages | Domain treatment |
|---|---|---|---|
| Streaming text/system/history | Supported | Experimental | Common |
| Usage input/output | Supported | Experimental | Common known fields |
| Usage total/cache/reasoning | Transformable | Experimental | Preserve unknown and categories |
| Stop/length/tool finish | Supported | Experimental | Common normalized reason |
| Single/parallel Tool call | Supported | Experimental | Common; application executes |
| Tool result | Supported | Experimental | Common paired result |
| HTTPS/inline image | Supported | Experimental | Common validated source; no download |
| JSON Schema output | Supported | Experimental | Capability-gated common format |
| Visible thinking | Unsupported by official adapter | Experimental | Common content when visible |
| Signature/redacted thinking | ProtocolOnly | Experimental | Opaque same-source replay only |
| Adaptive thinking/display/effort | ProtocolOnly | Experimental | Typed Anthropic options |
| Raw body extension | Unsupported | Experimental | Dangerous bounded Anthropic-only option |
| Automatic beta opt-in | Unsupported | Unsupported | Explicit future review required |
| Automatic protocol fallback | Unsupported | Unsupported | Application/Gateway responsibility |

Official Anthropic is limited to Messages streaming generation. Batch, Files,
Models, Token Counting, Admin, Bedrock, Vertex, prompt-cache management,
citations/PDF, server Tools, Computer Use, MCP, Tool execution, Agent loops,
remote image downloads, pricing, and billing are outside this SDK phase.

The Anthropic catalog entries `claude-sonnet-5` and `claude-opus-5` are
experimental declarations reviewed on 2026-07-25 and expire on 2026-08-25 unless
reviewed again. Offline fixtures do not constitute real-provider verification.
