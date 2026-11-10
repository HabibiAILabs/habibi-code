# Habibi v0.1.0

First experimental release of Habibi, a native Rust coding agent and reusable library.

- Direct Codex and Claude subscription adapters with OAuth PKCE and hidden remote callback input.
- Full-screen terminal UI with styled conversations, compact tool summaries, scrolling, and multiline input.
- Live steering, queued follow-ups, cancellation, and recoverable/resumable JSONL sessions.
- `/model`, `/settings`, configurable status bar, process RAM and context-budget readouts.
- Ask-per-tool permissions by default; explicit, reconfirmed YOLO auto-approval option.
- Linux x86-64/ARM64 static musl builds and macOS Intel/Apple Silicon builds.
- Checksum-verified, no-sudo installer and a best-effort background update notice.

## Install

```sh
curl -fsSL https://habibiailabs.github.io/habibi-code/install.sh | sh
```

The default destination is `~/.local/bin`. Add it to your PATH if prompted.
To pin this release, set `HABIBI_VERSION=v0.1.0` on the `sh` side of the pipe.

## Important limitations

Subscription adapters remain experimental: real-account authentication/inference has not been verified for this prototype. They are not provider-approved integrations, and there is no API-billing fallback. Claude compatibility uses Claude Code-specific identity. Review provider terms.

Tool approval is **not sandboxing**. Shell commands have your account's filesystem/network access. Sessions and credentials are plaintext files. YOLO auto-approves all tools and must be explicitly reconfirmed after resume.

macOS binaries are currently unsigned and not notarized. Updates are opt-in to install, never automatic. Release checksums detect corruption; they are not independent signatures.
