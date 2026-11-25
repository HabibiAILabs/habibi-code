# Habibi v0.7.5

Support larger MCP tool definitions without truncating descriptions or schemas.

- Replace the separate 16 KiB description and 64 KiB schema caps with a 1 MiB limit per serialized tool definition.
- Bound total tool metadata to 4 MiB across all servers in a connection attempt, checking definitions as discovery pages arrive.
- Identify the server, tool, actual byte count, and limit when metadata is rejected, without printing its contents.
- Add regression coverage for larger definitions, exact size boundaries, aggregate limits, and diagnostic privacy.

Existing configuration, OAuth credentials, and session journals remain compatible.
Restart after upgrading and send a chat message to retry MCP connections. Model
context limits still apply; live Datadog catalog compatibility has not yet been verified.

---

# Habibi v0.7.4

Fix the MCP initialization protocol mismatch.

- Explicitly advertise MCP protocol `2025-11-25` for the `initialize` handshake instead of inheriting the SDK's `2026-07-28` default, which uses a different startup lifecycle.
- Apply the compatible handshake to OAuth HTTP, bearer HTTP, and stdio connections.
- Identify the client as Habibi with its application version.
- Verify outgoing protocol and client identity in HTTP and OAuth integration tests.

Existing configuration, saved OAuth credentials, and session journals remain compatible.
Restart after upgrading, then send a chat message to retry MCP connections. This fixes
an identified client-side mismatch; live Datadog compatibility has not yet been verified.

---

# Habibi v0.7.3

Clarify MCP connection failures and what happens after authentication.

- Identify the failing server during connection initialization.
- Report safe handshake failure categories and available HTTP status codes instead of always recommending another OAuth login. Raw server payloads and credential-bearing URLs are not included.
- Explain that connections are attempted automatically when you send the next chat message, with MCP enabled in settings; no restart or separate reconnect action is required.
- Make clear that saving OAuth credentials does not verify the MCP connection.
- Add regression coverage for diagnostic privacy and authentication completion wording.

Existing configuration and session journals remain compatible. This improves diagnostics;
it does not claim to resolve every server-specific handshake failure.

---

# Habibi v0.7.2

Improve assistant Markdown rendering in the terminal without bundled syntax-highlighting grammars.

- Render headings, nested emphasis, strikethrough, links with visible URLs, blockquotes, rules, and task lists.
- Wrap lists with hanging indentation; display code blocks with language labels and a subtle background.
- Render basic tables with cell separators (aligned columns and syntax highlighting are not included).
- Scope Markdown to assistant response blocks so unfinished fences cannot affect later messages; keep user text and tool output literal.
- Use a lightweight event parser with default features disabled, no retained syntax tree or rendered-history cache, and the existing 256 KiB / 4,000-line transcript limits.
- Cover rendering, list wrapping, response isolation, and bounded metadata with integration tests.

Existing configuration and session journals remain compatible. Restart after upgrading;
resumed sessions use the new renderer automatically. Actual RSS overhead has not been benchmarked.

---

# Habibi v0.7.1

Remove the blanket tool-approval instruction from the default system prompt.

- Remove “Every tool requires user approval.” without adding a replacement.
- Keep runtime approval enforcement and Ask/YOLO behavior unchanged.
- Apply the new default to new sessions; resumed sessions retain their saved prompt.

Existing configuration and session journals remain compatible.

---

# Habibi v0.7.0

Native pi-secure workflows, with no extension installation or Node dependency.

- Memory-only session vault and built-in `request_secret`, `capture_secret`,
  `show_secret`, and `secure_exec`, subject to normal tool approvals.
- Private masked entry, confirmed local reveal/copy, 30-second display lifetime,
  clipboard stdin transport, and `/secret` browsing/show/clear commands.
- Buffered execution with exact-value redaction of all stored secrets, private
  stdout capture, bounded output/timeouts, process-group cleanup, and clear-time
  cancellation of running/queued secret operations.
- Headless list/clear support; private input/reveal fail closed without a TUI.
- Regression coverage for private UI behavior, capture failures, cancellation,
  redaction, approval, and journal/provider/resume hygiene across all adapters.

This is transcript hygiene, not a sandbox or protection from a malicious model.
No persistent secret storage, guaranteed memory zeroization, or automatic clipboard
clearing. Existing journals remain compatible; resuming always starts an empty
vault. See the README's **Private secrets** section for usage and limitations.

---

# Habibi v0.6.1

Fix the RAM status indicator on macOS.

- Read current-process resident memory using macOS `proc_pidinfo` instead of always displaying `RAM n/a`.
- Match Linux RSS semantics: current memory, not peak usage, with child processes excluded. No subprocesses or elevated permissions are needed.
- Preserve `n/a` as a safe fallback if the OS query fails.
- Add a runtime RSS regression test for Linux and both macOS CI architectures.

Existing configuration and session journals remain compatible. macOS binaries remain unsigned and not notarized.

## Install or upgrade

```sh
curl -fsSL https://habibiailabs.github.io/habibi-code/install.sh | sh
```

Restart Habibi after upgrading. Enable the RAM indicator in `/settings` if it is hidden.

---

# Habibi v0.6.0

Native image reading for vision-capable models.

- **Image attachments:** `read` detects PNG, JPEG, GIF, and WebP by signature and sends native image content through Codex, Claude, and Ollama adapters, rather than base64 text.
- **Model capabilities:** Ollama checks reported vision support before sending images. Hosted providers validate the selected model; a vision-capable model is required.
- **Code mode and resume:** approved child image reads are automatically forwarded to the model, and attachments persist in session journals. Code-mode attachments have an 8 MiB combined base64 limit.
- **Useful binary-file errors:** PDFs and other non-text files receive extraction guidance instead of opaque UTF-8 errors. Automatic PDF extraction is not included.
- **Preserved safeguards:** file approvals, workspace confinement, regular-file checks, and the 2 MiB file limit remain in place. Image context accounting uses a 16 KiB text-equivalent estimate per image.

Existing journals remain readable. Journals containing images include the image data and should be treated as sensitive. Library users constructing `ToolOutput` or `Message::ToolResult` must supply the new `images` field. Live provider vision testing is not included in automated validation. macOS binaries remain unsigned and not notarized.

## Install or upgrade

```sh
curl -fsSL https://habibiailabs.github.io/habibi-code/install.sh | sh
```

Restart Habibi after upgrading and select a vision-capable model to read images.

---

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
