# Providers and bundled models (v0.3.0)

Habibi keeps provider protocols in Rust and model choices bundled in the binary.
There is **no hosted catalog, automatic catalog download, or API-billing fallback**.
Custom model IDs are still accepted with `--model` or by typing in `/model`.

## Subscription snapshot

Checked on **2026-11-11**:

- Codex source: [Pi's `openai-codex` provider definition](https://github.com/badlogic/pi-mono/blob/main/packages/ai/catalog/providers/openai-codex.ts).
  This is explicitly a ChatGPT OAuth list, not OpenAI's separately billed API list.
  Bundled choices: `gpt-6.1-sol` (default), `gpt-6.1-terra`, `gpt-6-astra`,
  `gpt-6-sol`, `gpt-6-luna`, `gpt-5.6-luna`, `gpt-5.6-sol`, `gpt-5.6-terra`.
  Pi explicitly excludes `gpt-5.5` and `gpt-5.3-codex-spark` from this list because
  its source reports rejection by the ChatGPT-account backend. Habibi does not
  block typed IDs, so account-specific access can still be tried explicitly.
- Claude sources: [official model overview](https://platform.claude.com/docs/en/about-claude/models/overview),
  [Pi's Anthropic provider](https://github.com/badlogic/pi-mono/blob/main/packages/ai/catalog/providers/anthropic.ts),
  and [models.dev](https://models.dev/api.json).
  Bundled choices: `claude-sonnet-5-5` (default), `claude-opus-5-5`,
  `claude-haiku-5-5`, `claude-fable-5-2`.
  These are documented model IDs, not proof that every subscription or OAuth client
  can access them. The direct Claude subscription adapter remains experimental.

Existing user defaults and resumed sessions keep their selected IDs. A new bundled
default does not silently rewrite them. `habibi models` lists bundled choices without
network or authentication. `/model` uses Ctrl-N/P to cycle choices; typing remains
available. Provider changes require a new session to avoid replaying incompatible
native conversation blocks.

Live subscription authentication/inference was **not** exercised for this update.
Before publishing, test the intended models with real accounts; do not interpret
fixture tests or a catalog entry as account-access verification. Rejected requests
never fall back to API-key billing.

## Ollama

The native `/api/chat` adapter streams NDJSON, preserves assistant thinking and tool
calls in native history, and submits tool results with `tool_name`. Agent journal
IDs are generated locally because native Ollama does not require tool-call IDs.
Responses must finish successfully before tools are executed; disconnects, malformed
arguments, error events, and length-limited responses are rejected. Tool approval,
context compaction, steering, and cancellation use the same engine as subscriptions.

- Default URL: `http://127.0.0.1:11434`; default model: `qwen3:8b`.
- `--ollama-url` overrides `OLLAMA_HOST` (full HTTP(S) URL), which overrides the saved
  user URL. Resume uses the journaled URL regardless of environment changes.
- `habibi models ollama`, `/models`, and `/model` explicitly query `/api/tags`.
  The list is installed models, not a guarantee of tool support. No automatic pulls.
- No OAuth files or subscription credentials are used. Authenticated Ollama servers
  are not supported. URL credentials, queries, fragments, and redirects are rejected.
- Requests send workspace content to the configured server. Use only a trusted host;
  prefer HTTPS for remote servers outside a trusted local network.
- Model discovery is bounded to 4 MiB; chat responses to 32 MiB; unterminated stream
  frames to 8 MiB. Requests have timeouts and streamed chat has a 90-second idle limit.

A live local read-tool round trip was tested with the already-installed `qwen3:8b`.
Repeat explicitly (never part of the ordinary offline test suite):

```sh
HABIBI_TEST_OLLAMA_MODEL=qwen3:8b \
  cargo test -p libhabibi live_ollama_tool_loop -- --ignored --nocapture
```

The smoke test permits only `read` on its temporary fixture and denies other tools.
It does not download models or read subscription credentials.
