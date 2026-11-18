# Habibi v0.5.0

A more terminal-like prompt editor and a folder-scoped session browser.

- **Session picker:** `/resume` opens a searchable, newest-first list for the current folder, with original first-user/latest-message previews. Arrow keys or wheel select, Enter resumes, Esc closes. Narrow terminals stack the panes. Browsing is read-only; locked/invalid sessions leave the current session intact. Saved model and permissions are restored, and local control clients must rediscover the new socket. Preview/file-count limits are documented in the README.
- **Safer Ctrl-C:** clears a nonempty draft first, without quitting or interrupting the agent. With an empty prompt it cancels an active run or quits when idle. Esc still interrupts immediately.
- **Editable prompt history:** Up/Down or Ctrl-P/N browses up to 100 submitted prompts, including queued messages and commands. Down past the newest restores the unfinished draft and cursor. Available user messages seed history on resume. Autocomplete retains Up/Down priority while open; Ctrl-N retains approval-denial priority.
- **Terminal editing:** Ctrl-A/E or Home/End for line boundaries; Ctrl-B/F for characters; Alt-B/F or Ctrl/Alt-arrow keys for words; Ctrl-W and Alt-D for word deletion; Ctrl-K for deleting to line end; Ctrl-D/H for delete/backspace. Ctrl-U still clears the whole draft. Unicode character boundaries are preserved.
- **Select and copy:** mouse scrolling remains enabled, with no `/mouse` command. Use your terminal's selection override (commonly Shift-drag), then its copy shortcut. Ctrl-C is not copy. Selection remains terminal-owned, not a whole-session export.

Existing configuration and session journals remain compatible. No new global history file is written. Terminal modifier support varies. macOS binaries remain unsigned and not notarized.

## Install or upgrade

```sh
curl -fsSL https://habibiailabs.github.io/habibi-code/install.sh | sh
```

Restart Habibi after upgrading.

---

# Habibi v0.4.1

MCP OAuth from the terminal UI and CLI.

## What's new

- **Authenticate from `/mcp`:** select a saved HTTP server and press **a**. Browser authorization runs without blocking the terminal; the dialog shows a fallback link and accepts a privately pasted full callback URL. Esc or Ctrl-C cancels. Press **l** to remove local credentials. The server list shows saved OAuth versus explicit bearer authentication.
- **CLI authentication:** `habibi mcp login|status|logout SERVER`, with login options `--client-id`, repeatable `--scope`, and `--callback-port` for pre-registered public clients.
- **Automatic refresh:** Streamable HTTP connections use endpoint-bound saved OAuth when no bearer environment variable is configured. Expired tokens refresh through the MCP SDK, with cross-process locking and atomic private credential storage.
- **Protocol protections:** discovery, S256 PKCE, callback state/origin/duplicate-parameter checks, issuer binding on every credential read, and bounded callback input. Provider response bodies are not exposed through OAuth/transport error messages.
- **Library opt-in:** `mcp_oauth::{Store, Login, LoginOptions}` and `mcp::connect_with_oauth_store`; existing callers of `mcp::connect` keep their previous behavior.

## Compatibility and limits

Existing server configuration and journals remain compatible. Explicit bearer-env configuration takes precedence. OAuth requires published metadata with an issuer and a public client (dynamic registration or a supplied client ID); confidential clients and legacy HTTP+SSE are not supported. Login never starts automatically from a model/tool request. Credentials live in private files, not an encrypted keychain. Logout deletes local credentials, not provider grants, and cannot recall in-flight requests. macOS binaries remain unsigned and not notarized.

## Validation

Hermetic tests exercise discovery, dynamic/public registration, PKCE token exchange, loopback callbacks, automatic refresh during MCP initialization, issuer changes, endpoint isolation, logout, cancellation/denial, file permissions, lock contention, and hidden UI input. Full workspace tests, Clippy, Rust 1.89 compatibility, installer checks, and terminal smoke tests run before release. Live third-party OAuth accounts have not been tested.

## Install or upgrade

```sh
curl -fsSL https://habibiailabs.github.io/habibi-code/install.sh | sh
```

Restart Habibi, open `/mcp`, select a saved HTTP server with no bearer environment field, and press **a**.

---

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
