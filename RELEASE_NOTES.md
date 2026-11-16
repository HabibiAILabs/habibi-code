# Habibi v0.4.0

Local agent control, headless workflows, MCP, and embedded code mode.

## What's new

- **Control any running instance:** the TUI, `-p`, and `--rpc` each expose a private Unix socket. Send JSON Lines requests for status, prompts, steering, follow-ups, approvals, stop, and shutdown without taking over the terminal. Startup output shows the path; discovery metadata identifies the process and session. Client disconnects leave work running.
- **Headless and stdio RPC:** `habibi -p "…"` streams text and exits; `habibi --rpc` serves a persistent event/request protocol. Explicit `--yolo` is required for headless auto-approval regardless of saved TUI preferences. Stdio RPC supports individual tool approvals.
- **MCP integration and editor:** connect trusted stdio and Streamable HTTP servers from user configuration. `/mcp` offers forms for server arguments, environment references, URLs, enable/disable, and removal, with atomic saves and external-change detection. Connections start on the first prompt; repository MCP configuration is never loaded.
- **Embedded code mode:** QuickJS scripts compose registered tools with bounded parallelism, output, execution time, and memory. Child calls retain individual Ask/YOLO approvals and audit records. No external JavaScript runtime is required.
- **Extensible library tools:** register custom tools with validated input schemas and connect MCP servers through `libhabibi`.
- **Transient provider retries:** Codex, Claude, and Ollama retry connection failures, timeouts, and selected transient HTTP errors up to five times with cancellable backoff. Partial streams are not retried; tool calls are not replayed.
- **Command and skill completion:** a five-row list above the prompt includes discovered `/skill:name` entries. Up/Down selects, Tab completes, and partial matches can be accepted with Enter. Loading skill bodies remains approval-gated.
- **Documentation and GitHub Pages:** updated getting-started guidance, release information, and the local RPC protocol reference.

## Compatibility and safety

Existing journals remain readable. Journals containing new code-mode child-call audit records require v0.4.0 or later; older binaries cannot read those records. MCP/code-mode preferences persist in sessions and user defaults; both features are enabled by default. MCP programs run only when explicitly configured, but starting configured programs does not require a tool approval.

Socket directories are created with mode 0700 and sockets with mode 0600. Other processes running as your user can control the agent, including approving tools. Socket transport is request/reply only, not an event stream. Plain `-p` remains one-shot and denies tools unless `--yolo` is explicit. Normal exit removes discovery files; forced termination may leave stale entries.

Tool approval is not sandboxing: shell commands and MCP programs retain your account's filesystem/network access. Code-mode limits do not restrict the capabilities of approved tools. Cancellation cannot undo completed local or remote side effects. Saved TUI YOLO defaults apply to future new TUI sessions in every directory.

Subscription adapters remain experimental and are not verified provider-approved integrations. macOS binaries remain unsigned and not notarized.

## Validation

Release publication is gated on builds, workspace tests, Clippy, installer tests, and Linux terminal smoke tests across the four supported targets. The main-branch CI also checks the declared minimum Rust version. Tests exercise socket permissions, request bounds, disconnect behavior, cleanup, and shutdown acknowledgements, as well as RPC approvals, steering, cancellation, MCP/code mode, and provider retries. Live subscription authentication/inference is not validated; the live Ollama test is ignored by default.

## Install or upgrade

```sh
curl -fsSL https://habibiailabs.github.io/habibi-code/install.sh | sh
```

The default destination is `~/.local/bin`. Restart Habibi after upgrading. To pin this release:

```sh
curl -fsSL https://habibiailabs.github.io/habibi-code/install.sh | HABIBI_VERSION=v0.4.0 sh
```

Review the installer before running remote shell code. Checksums detect corruption; they are not independent publisher signatures.
