# Habibi v0.2.0

Agent Skills, inspectable context, and a more usable native coding loop.

## What's new

- Agent Skills discovery from configurable directories, defaulting to `~/.agents` and workspace `.agents`, including their `skills/` subdirectories. Frontmatter is added to context automatically; the approval-gated `load_skill` tool reads full instructions on demand.
- Sticky skills enabled by default: approved snapshots survive compaction and resume. Toggle sticky mode and edit the directory list in `/settings`; repeated `--skill-dir` options override the defaults for new sessions.
- `/inspect` read-only modal with searchable Context and Tool calls tabs. Browse active context and byte-budget breakdowns, tool arguments/results/errors, and latest/all session calls—even across compaction and resume.
- Manual `/compact` and automatic context summarization (80% of the byte budget by default; 0 disables it). Original journal history is retained; failed/cancelled compaction leaves active context unchanged.
- Bouncing-ball activity indicator in the prompt border for thinking, responses, tools, and compaction, without a duplicate Thinking footer label.
- Unlimited model turns by default. Set a positive cap in `/settings`, or use 0 for unlimited; existing sessions keep their saved cap.
- Saved YOLO mode automatically becomes active on session resume without repeated confirmation. New sessions still default to Ask, and initially enabling YOLO requires confirmation. **YOLO auto-approves every tool, including unsandboxed shell commands.**

## Compatibility

Existing session journals remain readable. New settings and tool-history checkpoints are written by this version; use v0.2.0 or later to resume journals updated by this release. Settings are session-local, not global preferences.

## Install

```sh
curl -fsSL https://habibiailabs.github.io/habibi-code/install.sh | sh
```

The default destination is `~/.local/bin`. Add it to your PATH if prompted.
To pin this release, set `HABIBI_VERSION=v0.2.0` on the `sh` side of the pipe.

## Important limitations

Subscription adapters remain experimental: real-account authentication/inference has not been verified for this prototype. They are not provider-approved integrations, and there is no API-billing fallback. Claude compatibility uses Claude Code-specific identity. Review provider terms.

Tool approval is **not sandboxing**. Shell commands have your account's filesystem/network access. Sessions and credentials are plaintext files. YOLO auto-approves all tools and remains active after session resume until disabled in /settings.

macOS binaries are currently unsigned and not notarized. Updates are opt-in to install, never automatic. Release checksums detect corruption; they are not independent signatures.
