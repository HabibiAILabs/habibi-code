# Habibi v0.3.1

Slash commands without interrupting the agent, plus command suggestions and Tab completion.

## What's new

- **Commands during execution:** `/inspect`, `/settings`, `/model`, `/models`, `/help`, and `/update` remain available while the agent runs. Commands are handled locally, not sent as steering messages.
- **Slash-command suggestions:** type `/` to see matching commands. Press **Tab** to complete the first match and keep pressing to cycle through matches. Suggestions narrow as you type.
- **Inspection during execution:** browse the latest read-only context-boundary snapshot while the agent continues working. Close and reopen the inspector to refresh. Inspection never executes or approves tools.
- **Deferred settings:** model and settings changes saved during execution apply after the current run finishes, before queued follow-ups. In-flight requests and current tool permissions remain unchanged, including Ask/YOLO policy. User defaults are saved when the changes are applied.
- **Documentation and website:** refreshed command guidance and GitHub Pages release information.

## Compatibility and safety

Existing journals and user defaults remain compatible. `/compact`, `/quit`, and `/exit` still require an idle agent; Esc interrupts a run. Closing an inspector or settings dialog does not cancel the run. The inspector is a snapshot, not a continuously updating view.

Subscription adapters remain experimental and are not provider-approved integrations. Tool approval is not sandboxing: shell commands retain your account's filesystem and network access. Explicitly enabling YOLO also saves that preference for future new sessions in every directory. macOS binaries remain unsigned and not notarized.

## Validation

Workspace tests cover slash-command matching and read-only inspection snapshots, alongside the existing provider, approval, steering, compaction, resume, settings, and terminal-status tests. The live Ollama test requires an external server and is skipped by default. Release publication is gated on the four-platform build/test workflow. Live subscription authentication and inference are not verified by these checks.

## Install or upgrade

```sh
curl -fsSL https://habibiailabs.github.io/habibi-code/install.sh | sh
```

The default destination is `~/.local/bin`. Restart Habibi after upgrading. To pin this release:

```sh
curl -fsSL https://habibiailabs.github.io/habibi-code/install.sh | HABIBI_VERSION=v0.3.1 sh
```

Review the installer before running remote shell code. Checksums detect corruption; they are not independent publisher signatures.
