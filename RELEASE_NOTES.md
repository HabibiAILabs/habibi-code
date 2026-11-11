# Habibi v0.3.0

Local Ollama, refreshed subscription choices, persistent defaults, and terminal status integration.

## What's new

- **Ollama provider:** native streaming chat and tool calls with the usual approval, steering, compaction, cancellation, and session-resume behavior. No OAuth, no automatic model downloads, and no subscription credentials sent to Ollama. Start with `habibi --provider ollama --model qwen3:8b` after installing the model yourself.
- **Model discovery and selection:** `habibi models` lists bundled subscription choices; `habibi models ollama` lists installed Ollama models. `/models` does the same for the active provider, and Ctrl-N/P in `/model` cycles choices. Custom IDs remain supported.
- **Subscription model refresh:** new-install defaults are `gpt-6.1-sol` for Codex and `claude-sonnet-5-5` for Claude. Bundled alternatives include the current GPT-6.1/GPT-6/GPT-5.6 ChatGPT OAuth choices and Claude Opus/Sonnet/Haiku 5.5 plus Fable 5.2. The snapshot was checked on 2026-11-11; account access varies. Existing saved model IDs are preserved. No hosted catalog or automatic catalog refresh.
- **Persistent user defaults:** applying `/model` saves a model per provider and the preferred provider. Applying `/settings` also saves frontend preferences, including explicitly enabled YOLO, for new sessions in every directory. Stored privately at `$XDG_CONFIG_HOME/habibi/settings.json` or `~/.config/habibi/settings.json`. CLI overrides do not rewrite defaults; resumed sessions keep their journaled settings.
- **Program Status Protocol:** OSC 7501 reports idle, working, permission/authentication blocked, done, error, and cancellation states for Rex and other supporting terminals. Messages are fixed and contain no prompts, arguments, paths, credentials, or provider error details. Duplicate states are suppressed. Set `HABIBI_NO_PROGRAM_STATUS=1` to disable.

## Compatibility and safety

Old journals remain readable. Ollama journals require **v0.3.0 or later**. New sessions use user defaults; resumed sessions retain their own model, endpoint, and permission policy. Context/turn/skill settings remain session-local. Applying `/model` alone never promotes an old session's permission mode to global defaults.

**Enabling YOLO in `/settings` now also enables it for future new sessions in every directory.** The confirmation explicitly warns about this scope. Disable it and Apply to restore Ask for future sessions. Existing journals keep their saved permission mode and must be changed separately. YOLO auto-approves all tools, including unsandboxed shell commands.

Remote Ollama endpoints receive conversation and workspace content. Use only trusted servers and HTTPS outside trusted local networks. Authenticated Ollama servers are not supported.

## Validation

Offline tests cover all three providers' tool loops, approval/denial, steering, compaction, native history, and resume; Ollama framing is tested across arbitrary UTF-8 chunk boundaries. A live local `qwen3:8b` read-tool round trip passed. OSC 7501 wire format, state mapping, deduplication, opt-out behavior, and private-data exclusion have unit coverage. An end-to-end pseudo-terminal test verified idle → working → blocked for tool approval → working → done → idle against an Ollama fixture server.

Live subscription authentication/inference and Rex's UI presentation have not been verified. Catalog references establish documented model IDs, not provider approval or guaranteed subscription access. See [provider notes](docs/providers.md).

## Install

```sh
curl -fsSL https://habibiailabs.github.io/habibi-code/install.sh | sh
```

The default destination is `~/.local/bin`. Add it to PATH if prompted. After this release is published, pin it with `HABIBI_VERSION=v0.3.0` on the `sh` side of the pipe.

## Important limitations

Subscription adapters remain experimental and are not provider-approved integrations. Claude compatibility uses Claude Code-specific identity. There is no API-billing fallback. Review provider terms.

Tool approval is **not sandboxing**. Shell commands have your account's filesystem/network access. Sessions, user defaults, and credentials are plaintext files. macOS binaries are unsigned and not notarized. Updates are opt-in to install, never automatic. Checksums detect corruption; they are not independent signatures.
