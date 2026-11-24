# Habibi

A small, native Rust coding-agent prototype: **`libhabibi` owns the agent loop; `habibi` is its full-screen terminal frontend.** No external JavaScript runtime, Habibi daemon, or provider-agent subprocess (Ollama runs as a separate server). Optional code mode embeds QuickJS; explicitly configured MCP servers may run as child processes. Without MCP connections, a best-effort background GitHub release check is the only idle network activity; it never contacts model providers.

## What's new in v0.7.3

MCP connection failures now identify the server and provide safe failure categories
rather than always suggesting another OAuth login. Authentication completion explains
that the connection is not yet tested: close `/mcp` and send a chat message to connect
automatically, with MCP enabled in `/settings`. No separate reconnect action is needed.

## Previous release: v0.7.2

Lightweight assistant Markdown rendering: nested emphasis, readable links, hanging-indent
lists, task checkboxes, blockquotes, rules, and labeled code blocks. Tables have basic
cell separators; aligned columns and syntax highlighting are not included. Markdown
is isolated per assistant response and also applies to resumed sessions after restart.
The transcript remains bounded; no syntax-highlighting grammar database or rendered-history
cache is added.

## Previous release: v0.7.1

Removed the blanket tool-approval sentence from the default system prompt.
Runtime approval checks are unchanged. This affects new sessions; resumed sessions
retain their saved system prompt.

## Previous release: v0.7.0

**Native private secrets**, based on [moejay/pi-secure](https://github.com/moejay/pi-secure):
masked entry, private capture, confirmed local reveal/copy, redacted shell execution,
and `/secret` browsing. No extension or Node runtime required.
See [Private secrets](#private-secrets).

## Previous release: v0.6.1

The RAM status indicator now works on macOS as well as Linux, using native
current-process resident memory reporting without subprocesses or elevated permissions.

## Previous release: v0.6.0

`read` now attaches PNG, JPEG, GIF, and WebP images as native content for
vision-capable models, including through code mode and resumed sessions. Ollama
checks model vision support. PDFs and other binary formats receive extraction
guidance. See [Reading images and other files](#reading-images-and-other-files).

## Previous release: v0.5.0

[Release notes](RELEASE_NOTES.md) · [Quick-start website](https://habibiailabs.github.io/habibi-code/)

- **Session picker:** `/resume` browses sessions for the current workspace with a searchable newest-first list and first-user/latest-message preview.
- **Prompt editing:** Ctrl-C clears a nonempty draft first. Up/Down recalls editable prompts and restores your unfinished draft. Terminal-style line/word navigation and deletion shortcuts are supported. Mouse scrolling stays enabled; use terminal-native Shift-drag to select text.

### Included from v0.4.1

- **MCP OAuth:** select a saved HTTP server in `/mcp` and press **a** to authenticate in your browser, or use `habibi mcp login SERVER`. Private endpoint-bound credentials refresh automatically; **l** removes local OAuth credentials.

### Included from v0.4.0

- **Control any instance:** private Unix sockets let other processes steer the TUI, headless runs, and RPC instances without taking over their terminal.
- **Headless and RPC:** `-p` streams a one-shot response; `--rpc` provides persistent JSON Lines control with individual tool approvals.
- **MCP and code mode:** manage trusted local/remote MCP servers with `/mcp`; embedded JavaScript can compose tools with bounded concurrency and per-call approvals.
- **Provider retries:** transient connection and HTTP failures retry with cancellable backoff, without replaying partial streams.
- **Commands during execution:** `/inspect`, `/settings`, `/model`, `/models`, `/help`, and `/update` work without interrupting the agent. Inspection shows the latest context-boundary snapshot; close and reopen to refresh.
- **Command and skill autocomplete:** type `/` for a five-item list above the prompt (not in the status line). Up/Down selects; Tab completes. Discovered skills appear as `/skill:name`.
- **Safe settings changes:** changes saved during execution apply after the current run, before queued follow-ups. Active tool permissions stay unchanged. `/compact`, `/quit`, and `/exit` require idle.

### Also included from v0.3.0

- **Ollama:** native local chat streaming and tool calls, no OAuth or automatic model downloads.
- **Current subscription model choices:** bundled Codex/Claude lists, `/models` to browse,
  and Ctrl-N/P in `/model` to select. New-install defaults are `gpt-6.1-sol` and
  `claude-sonnet-5-5`. Existing saved model IDs are preserved.
- **Persistent user defaults:** default provider, model per provider, and frontend
  preferences (including explicitly enabled YOLO) follow you into new sessions in
  every directory. Resumed sessions retain their saved settings.
- **Rex / OSC 7501:** terminal status reports for working, approval/authentication waits,
  completion, errors, and cancellation.
- Agent Skills, `/inspect`, manual/automatic compaction, steering, and follow-ups
  remain available. Tools ask by default unless you have explicitly saved YOLO.

### Upgrading from v0.2.0

Rebuild or reinstall after the release is published. Existing journals remain readable.
Ollama journals require v0.3.0 or later; do not resume them with older binaries.
Apply `/model` or `/settings` once to establish user defaults; we do not silently
promote a resumed session's permission policy to global defaults.

**YOLO is now a user default for new sessions when you enable it in `/settings`.**
The confirmation explicitly warns that this applies in all directories. Disable
it and Apply to restore Ask for future sessions. Existing journals retain their
own saved permission mode, including on resume. Shell commands are unsandboxed.

## Install v0.3

Prebuilt Linux x86-64/ARM64 (static musl) and macOS 15+ Intel/Apple Silicon binaries are published on [GitHub Releases](https://github.com/habibiailabs/habibi-code/releases). macOS binaries are unsigned and not notarized.

```sh
curl -fsSL https://habibiailabs.github.io/habibi-code/install.sh | sh
```

The installer verifies SHA-256 checksums and atomically installs to `~/.local/bin` without sudo. Add that directory to PATH if prompted. [Review the installer](scripts/install.sh) before executing remote shell code. Checksums detect corruption; they are not independent publisher signatures.

```sh
# Pin a version or choose an absolute installation directory
curl -fsSL https://habibiailabs.github.io/habibi-code/install.sh | \
  HABIBI_VERSION=v0.3.0 HABIBI_INSTALL_DIR="$HOME/bin" sh
```

Run the same command to upgrade; remove the installed `habibi` binary to uninstall (saved sessions/credentials are not removed). The [website](https://habibiailabs.github.io/habibi-code/) has a quick start.

### Update notices

After the TUI paints, a background check of the public GitHub latest-stable-release endpoint can show `Update vX.Y.Z available · /update` in the footer. It has a three-second HTTP timeout, silently ignores failures, and caches results (including failed attempts) for 24 hours at `$XDG_CACHE_HOME/habibi/update.json` or `~/.cache/habibi/update.json`. GitHub receives a normal HTTPS request with the app version/user agent and your IP address; no provider credentials, prompts, or workspace content are sent. Updates are never installed automatically.

Set `HABIBI_NO_UPDATE_CHECK=1` to disable both the check and cache access. `--help`, `--version`, login, and logout never check for updates. `/update` displays the upgrade command and release URL.

## Run

```sh
cargo build --release -p habibi

# ChatGPT subscription / Codex model access
target/release/habibi login codex
target/release/habibi --provider codex

# Claude Pro/Max subscription model access — experimental; see limitations below
target/release/habibi login claude
target/release/habibi --provider claude
```

Login prints a browser URL and waits up to five minutes for either a loopback callback or privately pasted callback input. Open that URL yourself. Codex uses port 1455; Claude prefers 53692 and can fall back to a free port.

**Remote/SSH login:**

1. Run `habibi login codex` (or `claude`) on the machine running Habibi.
2. Open the printed login URL in your local browser and authorize the account.
3. If the browser cannot reach `localhost`, copy the **full failed callback URL** from its address bar. Paste it into Habibi's waiting terminal and press Enter. Input is hidden, not echoed or stored in session history.

The local callback listener remains active while waiting for pasted input. `code#state` is also accepted; a bare code is rejected because it cannot verify OAuth state. The redirect address and state must match this specific login attempt. Esc or Ctrl-C cancels; Ctrl-U clears the hidden input. For a stale or invalid callback, restart login and use the newly printed URL. Never paste callback URLs or authorization codes into chat.

```sh
target/release/habibi --provider codex --model gpt-6.1-sol --cwd /path/to/project
target/release/habibi --provider claude --model claude-sonnet-5-5
target/release/habibi --resume /path/to/session.jsonl
target/release/habibi logout codex
```

**Resume without leaving Habibi:** run `/resume` while idle. The picker shows only
sessions associated with the current canonical workspace folder, newest first.
Use **Up/Down** or the mouse wheel to select; type to search by first message,
model, or session ID; **PgUp/PgDn** scrolls the preview; **Enter** resumes;
**Esc/Ctrl-C** closes without switching. The current session is marked with a dot.
The preview shows the original first user message and last recorded message
(including tool results), even across compaction. Wide terminals show list and
preview side by side; narrow terminals stack them.

Browsing is read-only and runs in the background. It scans the standard data
session directory and the current journal's directory (not arbitrary workspace
files), considering the newest 1,000 journal files. Invalid headers and symlinks
are skipped. Previews use the first/last 256 KiB and truncate individual messages;
large records may be unavailable, explicitly noted in the UI. Selecting a locked
or invalid journal reports an error without dropping the current session.
Switching restores saved settings and permissions (including YOLO), leaves the
old journal intact, and replaces the local control socket/identity. Socket clients
must rediscover the instance. CLI `--resume PATH` remains available for other
folders or journals outside the picker limits.

`--resume` restores the saved model, provider, workspace, and history. It cannot be combined with model/workspace overrides. MCP/code-mode preferences are restored on resume. Model availability depends on your account; use `--model` to override the defaults.

### Headless and RPC (experimental)

```sh
habibi -p 'Explain the architecture'             # stream text, then exit
habibi -p 'Implement the fix and run tests' --yolo # explicitly trust all tools
habibi --rpc                                    # persistent JSON Lines RPC
habibi --rpc -p 'Start investigating this bug'
```

`-p` / `--prompt` runs without a terminal. Output goes to stdout; journal location
and diagnostics go to stderr. Tools are denied unless `--yolo` is explicitly
passed; saved TUI YOLO settings do not silently enable headless approvals.
RPC clients can answer individual approval requests and send `prompt`, `steer`,
`follow_up`, `stop`, `status`, and `shutdown` requests. Stdio RPC uses stdin/stdout pipes. A separate private Unix socket also controls
every running instance, including the TUI (see below).
See **[the RPC protocol and client example](docs/rpc.md)** for request/event schemas,
approval handling, cancellation, and lifecycle details.

### MCP and code mode (experimental)

MCP and code mode are enabled by default, independently toggleable in `/settings`.
Preferences persist in the session and user defaults. MCP reads only
`$XDG_CONFIG_HOME/habibi/mcp.json` (default `~/.config/habibi/mcp.json`);
a missing file is normal. Connections start on the first prompt (not while idle).
Disabling MCP drops its connections; enabling it reconnects on the next prompt. Settings changed during a run apply afterward.

Open **`/mcp`** while idle to manage individual servers:

- **n** adds a server; **e / Enter** opens a colored form with labeled input boxes.
  No JSON editing: Habibi generates the configuration from the fields.
- Set **name**, **transport**, and **enabled** state. **Space / Left / Right** changes
  a selector; **F2** switches between local stdio and remote Streamable HTTP.
- Local servers have an **executable**, individual **argument** fields, and pairs
  of **child environment variable / parent source variable** fields. **F3** adds
  an argument; **F4** adds an environment mapping; **Ctrl-D** removes the focused
  argument or mapping. Each argument is passed exactly as entered—no shell quoting
  or splitting, including arguments containing spaces.
- Remote servers have a **URL** and optional **bearer-token environment variable**
  field. Enter the name of a parent environment variable, never a literal secret.
- **Tab / Up / Down** switches fields; **Left / Right / Home / End** moves the text
  cursor; **Ctrl-U** clears a field. Backspace, Delete, and paste are supported.
  The form scrolls to the focused field. Enter accepts the draft; Esc discards it.
- **Space** enables/disables the selected server (`"disabled": true` persists it).
- **a** starts **Authenticate** for the selected saved HTTP server; leave the bearer
  environment field blank for OAuth. Habibi opens your browser and displays a
  fallback link. A remote-browser callback can be pasted into the dialog (input
  hidden). **Esc / Ctrl-C** cancels without replacing existing credentials.
- **l** removes the selected server's local OAuth credentials (not provider-side
  revocation). The list distinguishes saved OAuth, bearer-env, and unauthenticated
  configuration; a saved credential indicator is not a live validity check.
- **d** removes the selected server from the draft (credentials are not deleted).
- **s**, then **Enter** confirms saving the user config and trusting its enabled
  programs. Esc closes the manager without saving. Existing connections close;
  enabled servers reconnect on the next prompt.

Changes are validated and saved atomically in a private file. External changes
since the manager opened are rejected rather than silently overwritten. Use
`env_from` / `bearer_token_env` references instead of placing secrets in the editor.
The global `/settings` MCP toggle still controls all servers together. If connection
fails, `/mcp` remains available to fix or disable the server before retrying.

Installing this user configuration **trusts its programs and remote servers**:
configured programs start before the next model request without a tool approval
and are not sandboxed.
Habibi never auto-loads repository MCP configuration. Example:

```json
{
  "mcpServers": {
    "local": {
      "command": "/absolute/path/to/mcp-server",
      "args": ["--stdio"],
      "env_from": {"API_TOKEN": "MY_SERVER_TOKEN"}
    },
    "remote": {
      "url": "https://example.com/mcp",
      "bearer_token_env": "MY_REMOTE_TOKEN"
    }
  }
}
```

Replace these placeholders with your servers. `env_from` maps child variable
names to parent environment-variable names, not secret values. Stdio children
receive a minimal environment (PATH, HOME, USER, TMPDIR, TEMP, SYSTEMROOT) plus
these explicit mappings. Their working directory is the session workspace;
stderr is discarded to avoid corrupting the TUI. Remote servers use Streamable
HTTP, require HTTPS except on loopback, and do not follow redirects. Bearer auth
is optional and takes precedence over saved OAuth. Legacy HTTP+SSE transport is
not implemented.

### MCP OAuth

Save your HTTP server with an empty bearer environment field, then reopen `/mcp`
and press **a**, or run:

```sh
habibi mcp login remote
habibi mcp status remote
habibi mcp logout remote
# For servers without dynamic public-client registration:
habibi mcp login remote --client-id YOUR_PUBLIC_CLIENT_ID --callback-port 8765 --scope tools
```

The flow discovers protected-resource / authorization-server metadata, uses an
S256 PKCE challenge and state validation, and binds a loopback callback before
opening the browser. Pre-registered clients must permit the displayed
`http://127.0.0.1:PORT/callback` redirect; omit `--callback-port` to allocate a free
port. `--scope` is repeatable; otherwise initial scopes come from discovery.
Only public clients are supported (no client secrets). Servers must publish OAuth
metadata with an issuer and support either dynamic public-client registration
or a client ID supplied through the CLI. No automatic login or browser opening
occurs during model/tool requests.

OAuth credentials are stored separately from server configuration and journals
under `$XDG_CONFIG_HOME/habibi/mcp-auth` (default `~/.config/habibi/mcp-auth`),
with 0700 directories and 0600 files. They are **not encrypted at rest**. Keys bind
to the full canonical server URL, not the display name; servers with the same URL
share credentials. Refreshes use cross-process file locks and atomic writes.
Automatic refresh runs on authenticated HTTP requests; an expired or rejected
refresh token requires a new explicit login. Authentication changes through
`/mcp` close existing connections; they reconnect on the next prompt. CLI logout
removes local credentials only; already in-flight requests may complete. Revoke
the application grant with the provider if needed. Never paste callbacks or
credentials into the agent chat.

Library hosts opt in with `mcp::connect_with_oauth_store` and supply the storage
path themselves. `mcp::connect` keeps its existing explicit-bearer behavior.
The OAuth SDK installs the ring rustls default only if the embedding process has
not already selected a crypto provider.

Tools are discovered on connection and named `mcp__SERVER__TOOL`. Server names use
1–24 ASCII letters, digits or hyphens; the complete tool identifier must fit
64 ASCII letters, digits, underscores or hyphens. Invalid/colliding definitions
fail connection rather than silently aliasing tools. MCP resources, prompts,
server-initiated sampling/elicitation, and live catalog refresh are not supported.
Tool errors and structured MCP results are preserved; image blocks are currently
JSON, not rendered images. Calls time out after 120 seconds; timeout/cancellation
cannot roll back remote side effects.

Code mode lets the **model** invoke `codemode({"code":"..."})`; it is not a
JavaScript prompt in the TUI. Scripts run as async function bodies:

```javascript
const files = ["Cargo.toml", "README.md"];
for (const path of files) {
  const content = await tools.read({ path });
  text({ path, preview: content.slice(0, 500) });
}
return "Inspected two files";
```

- Globals: `tools[name](args)`, `text(value)`, `console.log/info/warn/error`,
  `searchTools(query)` (simple case-insensitive term matching, up to 20 results),
  and `describeTool(name)` (description and JSON input schema).
- Built-ins return text; MCP tools return the full MCP result object, including
  `content` and `structuredContent`. Failed or denied tool calls reject.
- Script approval **does not approve its child calls**. Ask/YOLO applies to each
  child just as to direct tools. Calls and results appear in the TUI/inspector
  and session audit journal; only explicit script output enters model context.
- `Promise.all` / `Promise.allSettled` run up to eight tools concurrently, with
  serialized approval prompts. Writes/edits serialize to avoid lost updates;
  explicitly await dependent operations. No nested code mode, persistent
  script store, TypeScript, module imports, Node APIs, filesystem/network APIs,
  or timers. Use approved tools for external access.
- Limits: 64 MiB VM memory, 64 KiB source/output, 64 tool calls, 120 seconds
  including worker/approval waits, and two active workers per process. In-flight
  local mutations are awaited rather than abandoned when cancellation occurs. Completed
  side effects remain after script failure; partial output is retained.
- Tool catalogs are still exposed to the model normally. Deferred exposure and
  catalog optimization are follow-up work.

**Neither MCP servers nor `tools.bash()` are sandboxed.** The restricted JS VM
only limits direct script access; tool capabilities remain unchanged. External
tool registrations are not replayed from journals. Code-mode preferences are
restored; MCP connections are rebuilt from user configuration on the first prompt after resume. Journals with child-call audit records require this
version or newer; older binaries cannot read those new records.

Library hosts can use `Session::register_tool`, atomic `register_tools`,
`tool_definitions`, `enable_codemode`, and `mcp::connect`. Custom tools implement
`Tool` and receive a workspace and cancellation token. They are trusted host
code and must honor cancellation, bound output, and finish local mutations
before returning. Input schemas are validated without network/file `$ref`
resolution; registering a tool never grants permission to execute it.

### Ollama and model choices

```sh
# In another terminal, if Ollama is not already running:
ollama serve

# Download a tool-capable model yourself (Habibi never does this automatically):
ollama pull qwen3:8b
habibi models ollama
habibi --provider ollama --model qwen3:8b

# Optional remote server (the endpoint is saved in the session journal):
habibi --provider ollama --ollama-url http://your-server:11434 --model YOUR_MODEL
habibi models ollama --ollama-url http://your-server:11434

# Bundled subscription choices; no login/network needed to list:
habibi models
habibi models codex
habibi models claude
```

`/models` lists choices for the active provider. For Ollama, `/models` and `/model`
explicitly discover installed models from the configured server; discovery does not
load or download them. In the model field, **Ctrl-N/P** cycles choices, typing allows
any ID, and Enter applies and saves a default. Provider changes require a new session
because native conversation formats differ.

Ollama defaults to `http://127.0.0.1:11434` and `qwen3:8b`. URL precedence for new
sessions is `--ollama-url`, then `OLLAMA_HOST` (a full HTTP(S) URL), then the saved
Ollama URL, then localhost. Applying `/model` or `/settings` while using Ollama saves
its URL as well. Resume always uses the journaled URL, not environment overrides.
Remote Ollama servers receive prompts, file contents and tool results: use only a
trusted endpoint, with HTTPS outside trusted local networks. No subscription
credentials are sent to Ollama. Servers requiring authentication are not supported.

Bundled subscription choices were checked on **2026-11-11** against
[Pi's ChatGPT OAuth catalog](https://github.com/badlogic/pi-mono/blob/main/packages/ai/catalog/providers/openai-codex.ts)
and [Claude's model documentation](https://platform.claude.com/docs/en/about-claude/models/overview).
Codex choices use Pi's subscription-specific list, not OpenAI's API-billing catalog.
Claude names are documented model IDs, **not a guarantee of OAuth/account access**.
There is no hosted catalog or automatic catalog refresh. See [provider notes](docs/providers.md).

### Persistent user defaults

Applying `/model` saves the model as your default for that provider and selects that
provider for future launches. Applying `/settings` also saves frontend preferences,
including YOLO, for **new sessions in every directory**. Defaults are stored at
`$XDG_CONFIG_HOME/habibi/settings.json` (or `~/.config/habibi/settings.json`).
Model defaults are tracked separately for Codex, Claude, and Ollama.

Explicit `--provider` and `--model` flags override defaults for that launch without
rewriting them. Resumed sessions retain their journaled settings rather than taking
new user defaults. Other settings (context budget, turn limit, compaction and skills)
remain session-local.

**YOLO is a global opt-in for new sessions:** the confirmation warns that all tools
will be auto-approved in future sessions, regardless of directory. To turn it off
for future sessions, disable it in `/settings` and Apply. Existing sessions retain
their own saved permission mode; disable it separately when resuming those sessions.
Changing only `/model` never changes your global permission preferences.

### Agent Skills

Habibi discovers [Agent Skills](https://agentskills.io) at session creation and resume.
Put each skill in `<root>/<skill-name>/SKILL.md`, with YAML frontmatter containing
`name` (matching the directory) and `description`, followed by Markdown instructions.
The complete frontmatter (including optional fields) and skill path are automatically
included in model context; the body is only included after the model calls
`load_skill` with `{"name":"skill-name"}` and you approve it. Loading does not execute
scripts or automatically read referenced files. Skill content is untrusted reference
data, not a replacement for system instructions.

The default directory list is `["~/.agents", ".agents"]`. `~` expands to your home
and relative paths resolve against the workspace (`--cwd`, or the current directory).
Each entry supports both `<entry>/<skill-name>/SKILL.md` and the conventional
`<entry>/skills/<skill-name>/SKILL.md`. The latter wins within an entry; later entries
win over earlier entries, so workspace skills override home skills by default.
Discovery scans immediate child directories; malformed/unreadable skills are skipped.
Files must be UTF-8 and at most 2 MiB. No skills are downloaded automatically.

Edit **Skill directories (JSON list)** in `/settings`, for example:
`["~/.agents", ".agents", "~/.claude/skills", "./team-skills"]`.
The list is saved with the session and rediscovered immediately when you apply it.
Repeatable `--skill-dir` arguments replace the default list for a new session:

```sh
habibi --skill-dir '~/.agents' --skill-dir .agents --skill-dir ./team-skills
habibi --no-sticky-skills
```

**Sticky skills are ON by default.** Exact loaded snapshots stay outside compactable
history and survive `--resume`, even if the source changes or disappears. This does
not preload skill bodies. Toggle **Sticky skills** in `/settings`; when OFF, loaded
bodies are ordinary tool results that may be summarized. Approved snapshots are saved
so turning sticky back ON restores them without rereading changed files.
Metadata and active sticky snapshots count toward the context byte budget; compaction
cannot free those bytes. Increase the budget if retained skills leave too little room.
Options are session-local and saved with the session; resume does not accept overrides.

Library hosts can set `SessionConfig.skill_dirs` and `sticky_skills`; set
`discover_skills = false` to disable discovery entirely.

### TUI

- **Enter:** submit while idle; during a run, queue **steering** for the next safe model boundary (after the current response and its tool batch). Steering does not interrupt an in-flight HTTP request or tool.
- **Alt+Enter:** queue a **follow-up**, which starts a separate run when the current run finishes successfully. Steering takes priority over follow-ups. Both queues are bounded to 32 messages; a full queue leaves your draft intact. Queued-but-unapplied messages are in memory only; applied steering and started follow-ups are saved in conversation history.
- **Ctrl+Enter:** insert a newline. **Ctrl-J** is an alternate newline binding. Ctrl+Enter requires a terminal supporting enhanced keyboard reporting; on older terminals that send it as plain Enter, use Ctrl-J or paste multiline text. Input wraps, grows up to six visible rows, and scrolls to the cursor; Left/Right, Home/End, Delete, and Backspace edit the draft. Multiline paste preserves line breaks.
- **Ctrl-Y / Ctrl-N:** approve one proposed tool call or deny it. In the default Ask mode, every tool requires approval, including reads. **F2** opens the scrollable argument inspector, where **y/n** also approve/deny. Plain y/n in the composer is ordinary text, so drafting a message cannot accidentally approve a tool.
- **Esc:** interrupt the active run, including a pending approval, and clear queued messages. In settings it closes the dialog without saving. Already-completed tool effects are not undone. Errors also clear pending queues rather than automatically continuing.
- **Ctrl-C:** clear a nonempty prompt first (without cancelling the run); with an empty prompt, cancel an active run or quit when idle. Esc still interrupts immediately.
- **Prompt history:** Up/Down or Ctrl-P/N recalls the last 100 submitted prompts, including commands and queued messages; Down past the newest restores your unfinished draft and cursor. Recalled text is editable; history entries are unchanged. Available user messages seed history when resuming. Up/Down selects autocomplete when it is open; Esc dismisses completion when idle. Ctrl-N still denies a pending tool approval.
- **Cursor/editing:** Left/Right or Ctrl-B/F moves one character; Ctrl/Alt-Left/Right or Alt-B/F moves by whitespace-delimited words. Home/End or Ctrl-A/E moves to the current line's start/end. Ctrl-W or Ctrl/Alt-Backspace deletes the previous word; Alt-D or Ctrl/Alt-Delete deletes the next word. Ctrl-D deletes a character (never exits), Ctrl-H backspaces, Ctrl-K deletes to line end (or the newline at line end), and Ctrl-U clears the whole draft.
- **Select/copy:** mouse scrolling is always enabled. Use your terminal's mouse-selection override (commonly **Shift-drag**), then its copy shortcut (usually **Ctrl-Shift-C** on Linux or **Cmd-C** on macOS). Ctrl-C is not copy. Selection is terminal-owned and covers rendered text; modifier behavior varies by terminal.
- **PgUp / PgDn or mouse wheel:** scroll the transcript or open argument inspector. Scrolling up keeps your reading position stable as output arrives; scroll back to the bottom to follow output again. Conversation roles, headings, bold text, inline code, and fenced code blocks have lightweight styling. Tools/results use compact summaries, with full content retained in the session journal. Thinking is shown as a compact activity indicator, never raw private reasoning.
- **Slash autocomplete:** typing `/` shows up to five matching commands/skills above the prompt, with descriptions. **Up/Down** selects a row; **Tab** accepts it. **Enter** accepts a partial match, then submits the completed command on the next press; exact commands submit immediately. **Esc** hides suggestions while idle (during a run it still cancels). Narrow the prefix to find other commands.
- **Skills:** `/skill:name [request]` asks the agent to use that discovered skill. Skill names also match short prefixes such as `/rev` → `/skill:review`. Completion reads the session’s discovered metadata, refreshes when skill-directory settings change, and never loads skill bodies or bypasses `load_skill` approval. Skill requests can be steered or queued as follow-ups like ordinary prompts.
- **Ctrl-U:** clear input. **/help**, **/quit**, **/models**, **/model**, **/settings**, **/compact**, **/inspect**.
- **Activity animation:** while running, the prompt border shows a bouncing ball with **Thinking**, **Responding**, **Executing tool call: <name>**, or **Compacting context**. It pauses for approvals and stops when idle. The footer keeps safety/cancellation hints without repeating the activity label.
- **/inspect:** open a read-only context and tool-call inspector, including during execution (see below).
- **/models:** list bundled subscription choices or discover installed Ollama models.
- **/model:** choose with Ctrl-N/P or type a model ID in a modal; Ctrl-D restores the bundled provider default. Enter saves a default for future new sessions. Custom IDs are supported; availability depends on your account/server.
- **/settings:** edit model, maximum model turns per prompt (**0 = unlimited**, the default), context budget in bytes, auto-compaction percentage (default **80%**, **0** disables it), sticky skills, and the skill directory list. Tab/Up/Down selects a field, Ctrl-U clears it, Enter validates and applies, and Esc discards the draft. Model and frontend preferences also become user defaults; other settings remain session-local. `--resume` restores journaled settings; provider and workspace remain fixed. Changes saved during execution apply after the current run.
- **Status bar:** `/settings` lets you toggle activity, model, RAM, context, and keyboard hints individually using Space or Left/Right. Current-process resident RAM refreshes once a second on Linux and macOS (`n/a` on unsupported platforms or if the OS query fails); child-process memory is not included. Context reports serialized conversation plus skill-context bytes used / configured limit and percentage, **not provider tokens or the actual model context window**. Usage updates as conversation messages are committed, not per streamed token.
- **Tool permissions:** `/settings` selects **Ask every time** (default) or **YOLO** (auto-approve every tool, including reads, edits, writes, and unsandboxed shell commands). Enabling YOLO requires a second Enter to confirm the warning. The preference is saved with the session and **automatically restored on resume**, without another confirmation. New sessions default to Ask unless you explicitly saved YOLO in `/settings`. YOLO and active approval/cancellation indicators cannot be hidden by status-bar toggles. Only enable it for trusted work; shell commands retain your account’s filesystem/network access.

Approval previews over 24 KiB are explicitly truncated; do not approve a change you cannot evaluate. The composer is a bounded multiline editor, not a full terminal-editor implementation.

### Terminal status (Rex and OSC 7501)

Habibi supports the [Program Status Protocol](https://www.superlogical.com/rex/docs/build/program-status)
(OSC 7501, revision 0.4). The TUI reports idle, working (thinking/responding/tools/
compaction), blocked for tool approval, done, and error. Cancellation reports idle;
subscription login reports blocked with kind `auth`. Completion/error records remain
available for the terminal to show until acknowledged or replaced.

Reports use the `habibi` record ID and fixed, base64-encoded messages. No prompt,
response, tool arguments, file paths, credentials, or provider error details are sent
as status metadata. Repeated reports are deduplicated and no feature-detection query
is sent. Unsupported terminals should ignore the OSC. Reports are disabled when
stdout is not a terminal; set `HABIBI_NO_PROGRAM_STATUS=1` to disable them explicitly.
The library never emits terminal status sequences.

Slash commands remain available during execution. `/settings` and `/model` changes are queued until the current run finishes, before queued follow-ups; current tool permissions stay unchanged. `/compact`, `/quit`, and `/exit` require an idle agent.

### Context and tool-call inspector

Run **`/inspect`** at any time to open a two-tab modal. During execution it shows the latest context-boundary snapshot; the agent keeps running. Close and reopen to refresh:

- **Context:** budget usage with a proportional breakdown, system prompt, skills catalog
  and sticky snapshots, tool definitions, and each active conversation message.
  Sizes are bytes, not tokens. System prompt and schema overhead are outside the
  harness's conversation/skills budget. Provider-native blocks count toward message
  sizes but are not displayed; this is not an exact provider wire-payload viewer.
- **Tool calls:** shows calls since the latest user message by default, newest first.
  Press **a** to toggle all calls for the session, including calls before compaction.
  Select a call to view its arguments, recorded result, success/error status, and
  current tool definition. Recorded results retain the execution tool's own output
  limits. Calls without results are marked pending/interrupted. History survives
  resume and is kept outside model context; browsing it does not undo compaction.

**Tab**, **1**, or **2** switches tabs; **↑/↓** or **j/k** selects entries;
**PgUp/PgDn** or the mouse wheel scrolls details. **/** searches labels and content,
**Enter** finishes entering the search, and **c** clears it. **Esc/q** closes the
modal (Esc first leaves search input when searching). Inspection never executes or
approves tools and does not send requests to a provider. F2 remains the separate
approval inspector during a run.

## Private secrets

Habibi includes the workflows from [pi-secure](https://github.com/moejay/pi-secure)
natively. **Secrets for tools, not the transcript.** Ask “Request my API token
securely, then use it to call the service.” Enter the value **only in the masked
private dialog**, never in chat, slash commands, or tool arguments.

### Tools

All four tools use Habibi's usual approval policy, including when invoked from
code mode. Ask mode requires approval; explicitly enabled YOLO auto-approves.
The reveal/copy confirmations are separate and always remain local.

| Tool | Behavior |
| --- | --- |
| `request_secret({name, reason})` | Masked terminal input; Enter submits, Esc/Ctrl-C cancels. One asterisk per character (length is locally visible). Rejects empty, multiline and NUL input. Reuses an existing name. Returns name/availability only. |
| `capture_secret({name, command, secrets?, trimNewline?, timeout?})` | Captures shell stdout directly into memory; **neither stdout nor stderr is returned**, even on failure. Never overwrites an existing name. Optional `secrets` inject stored values as child environment variables. Removes one trailing LF/CRLF by default; `trimNewline: false` preserves it. Rejects nonzero exit, empty/NUL output and invalid UTF-8. |
| `show_secret({name})` | Opens a private local dialog, masked by default. **R** reveals/hides and **C** copies; both require **Y** confirmation. **N/Esc** cancels confirmation; Enter/Esc closes, Ctrl-C closes even while confirming. Auto-closes after 30 seconds, including while copying. Returns shown/copied status only. |
| `secure_exec({command, secrets, timeout?})` | Runs `/bin/sh` with named secrets in the child environment only. Buffers stdout/stderr and redacts exact values of **all** stored secrets, not just those injected. Returns exit code and redacted output. No streaming or raw-output temporary files. |

Names use `[A-Za-z_][A-Za-z0-9_]*`. Commands default to **60 seconds**, accept
`timeout` in seconds up to **300**, and enforce a **1 MiB combined output limit**.
Timeout, cancellation, overflow, signal termination, or invalid output encoding
discard output. Redaction also fails closed if unique stored values exceed its
16 MiB matching budget; clear unneeded captures before retrying. A failed command may already have had side effects; inspect those
before retrying. Shell execution requires Unix (Linux/macOS).

For example, the model can call:

```js
request_secret({ name: "APP_TOKEN", reason: "Authenticate the requested API call" })
secure_exec({
  command: 'curl -fsS -H "Authorization: Bearer $APP_TOKEN" https://api.example.com/account',
  secrets: ["APP_TOKEN"]
})
capture_secret({ name: "FILE_TOKEN", command: "cat /private/path/token" })
```

Commands, paths, reasons, names, availability, and redacted output are logged
normally. Use a command that emits only the intended value when capturing (for
example `curl -f` plus `jq -er .token`). **Never embed literal credentials in a
command** or read secret-bearing files through ordinary `read`/`bash` calls.

### Local commands and clipboard

- **`/secret list`** (or `/secret`) opens a names-only browser. **↑/↓ or j/k**
  selects; **Enter/R** opens the selected secret and returns to the browser on
  close. **D/Delete** then **Y** forgets it; **N/Esc** cancels deletion.
  **Esc/Ctrl-C** closes the browser.
- **`/secret show NAME`** opens an existing secret directly, without a model call.
- **`/secret clear NAME`** forgets one; **`/secret clear`** forgets all immediately.
  Clearing even one cancels all active secure operations and invalidates queued
  dialogs, preventing pending input/captures from restoring forgotten values.
  Other names remain stored after a single-name clear.

In headless mode, `habibi -p '/secret list'` reports names only. RPC/control
`prompt`/`steer`/`follow_up` text containing `/secret list` or `/secret clear [NAME]`
is handled locally, without sending it to a model. **Private entry and reveal/copy
require the interactive terminal**: no chat, RPC, or OSC 52 fallback.

Display escapes terminal control characters and truncates large values; copy uses
the full value. Clipboard helpers receive values through stdin, never arguments:
macOS `/usr/bin/pbcopy`, Linux Wayland `wl-copy`, or X11 `xclip`/`xsel`. Install a
helper separately if necessary. Unsupported environments fail closed.

### Storage and security boundary

Values live only in the session's memory, not journals, tool history, model
context, or serialized events. They survive turns and compaction but disappear
on clear, session switch/resume, shutdown or restart. Private dialogs are queued.
Memory zeroization is **not guaranteed**.

**Transcript hygiene, not a sandbox or secret manager.** A malicious model or one
following a prompt injection can still write a secret to a file, read it with
another tool, transmit it over the network, or transform it to evade exact-value
redaction. Other tools' output is not redacted. Approval and local reveal/copy
confirmation do not provide isolation. Use trusted tasks/destinations and
short-lived, least-privilege credentials; use external sandboxing/network policy
where necessary.

Terminal recordings, screenshots, process inspection, debug logs, crash dumps,
clipboard managers/sync, and the OS are outside this protection. Hiding a value
does not erase recordings. **The clipboard is not automatically cleared**; clear
it and its history after pasting. Forgetting a secret does not revoke it or erase
external copies. See the [pi-secure license notice](docs/pi-secure-LICENSE).

## Native core

- Direct streaming model requests to the ChatGPT Codex backend, Anthropic Messages endpoint, and native Ollama chat endpoint.
- OAuth PKCE, callback state checks, refresh-token rotation, and cross-process refresh locking.
- Built-in workspace tools: `read`, `write`, exact single-match `edit`, and `bash`. Native secret tools: `request_secret`, `capture_secret`, `show_secret`, and `secure_exec` (see above).
- Direct model tool calls run sequentially; code mode supports bounded parallel calls. File writes use atomic replacement. Reads/writes are limited to 2 MiB; shell output is captured, drained, and bounded to 64 KiB per stream.
- Cancellation drops HTTP requests and kills shell process groups on Unix, including descendants. Already-completed edits are not undone; in-progress atomic file operations finish before cancellation returns.
- Private append-only JSONL sessions, exclusive session locks, resume, torn-tail recovery, and interrupted-tool-result repair.
- Bounded event channels and terminal transcript. Terminal control characters are removed from displayed output.

Runs have no model-turn limit by default. Set a positive maximum in `/settings` to cap turns per prompt; **0** means unlimited. Existing saved sessions retain their configured limit. The default context limit is 1 MiB of serialized transcript, **not a token estimate**. Before a model turn, the agent automatically summarizes at the configured percentage (80% by default). Use **/compact** while idle to manually summarize, including after `habibi --resume SESSION.jsonl`, then submit your next prompt to continue. Compaction uses the selected provider/model with tools disabled and replaces active history with a continuation summary; original records remain in the journal. Settings and summary checkpoints survive resume. Summary generation is lossy and requires working provider access; a failed/cancelled summary leaves history unchanged. The byte budget is not the provider context window: if the provider already rejects the history as too large, summarization may also fail. With auto-compaction off, exceeding the budget stops the run explicitly.

### Transient provider errors

Codex, Claude, and Ollama calls automatically retry connection failures, request timeouts,
and HTTP 408, 429, 500, 502, 503, 504, and 529 responses up to **five retries after the
initial attempt** (six attempts total). Backoff is **2, 4, 8, 16, and 32 seconds**.
A numeric `Retry-After` can increase the delay, capped at 60 seconds; HTTP-date values
fall back to the normal schedule. Other HTTP errors (including authentication and
invalid requests) fail immediately.

The same policy covers compaction and explicit Ollama model discovery. The TUI reports
retries during agent runs and compaction; Esc/Ctrl-C cancels requests and backoff waits.
Once response headers have been accepted and streaming begins, stream failures and
provider stream-error events are **not** retried automatically, preventing replay of
partial output. Retries do not rerun tools or add conversation turns. Exhausted retries
surface the final request error normally.

### Subscription caveat

**These are experimental direct adapters, not verified provider-approved integrations. Live subscription authentication and inference have not yet been tested for this prototype.** Protocol support is based on direct integrations in Pi; it is not a promise that an account will authorize Habibi.

- No Codex or Claude Code agent is invoked. Habibi constructs model requests, executes tools, and owns conversation state.
- The OAuth adapters use public client identifiers used by existing coding clients, not separately registered Habibi OAuth applications.
- Claude's compatibility path sends Claude Code-specific identity headers and an identity system block, as existing direct adapters do. Habibi is **not** an official Anthropic client. This creates a provider-policy and compatibility dependency, not a stable public subscription SDK contract. Review current provider terms before use.
- Subscription access, provider authorization, rate limits, and model availability are controlled by the provider. Requests may be rejected or protocols may change.
- There is **no API-key fallback and no automatic switch to separately billed API usage**.
- Habibi does not import, read, or modify another agent's login cache. Login and refresh credentials are stored separately.

References: [Pi OAuth/provider implementation](https://github.com/earendil-works/pi/tree/main/packages/ai/src), [Codex authentication](https://developers.openai.com/codex/auth/), [Claude legal/compliance](https://code.claude.com/docs/en/legal-and-compliance).

### Security boundary

**Approval is not sandboxing.** Shell commands run with your user account's filesystem/network access and inherited environment. File tools reject resolved paths outside the workspace, but this is not OS-level isolation or a defense against concurrent filesystem races. Only approve trusted operations.

On Unix, newly created credential/session files use mode `0600`, and newly created storage directories use `0700`. Stored files are plaintext, not encrypted. Sessions can contain sensitive prompts, file contents, and tool results. Existing credential files accessible by group/others are rejected. Linux and macOS are the release targets; equivalent Windows permission and process-tree controls are not implemented.

Default storage:

- Credentials: `$XDG_CONFIG_HOME/habibi/auth/`, or `~/.config/habibi/auth/`.
- User defaults: `$XDG_CONFIG_HOME/habibi/settings.json`, or `~/.config/habibi/settings.json`.
- Sessions: `$XDG_DATA_HOME/habibi/sessions/`, or `~/.local/share/habibi/sessions/`.

`--help` and `--version` do not touch configuration. The idle TUI does not load credentials or contact a provider unless you explicitly request Ollama discovery with `/models` or `/model`. Its optional background release check only contacts GitHub (see Update notices above).

## Embed `libhabibi`

The library has no terminal dependency and does not print, exit the process, discover configuration, or choose a storage location. Hosts supply configuration and credential storage explicitly. Sessions are in memory unless you opt into persistence. For remote OAuth, `Login::complete_with_callback` accepts a future supplying private callback input while continuing to listen for the normal browser callback.

```rust,ignore
use libhabibi::{Session, SessionConfig, Provider, auth::AuthStore};

let config = SessionConfig::new(Provider::Codex, workspace);
let mut session = Session::new(config, AuthStore::new(auth_directory))?;
// Optional: session.persist(new_session_path)?;
```

`Session::prompt` owns the complete agent loop. Consume its typed `Event`s concurrently through a Tokio channel; display `TextDelta`s and answer `Permission` requests through their one-shot reply channel. Dropping a permission reply denies the tool. `CancellationToken` stops a run. Do not simply drop a prompt future and assume cancellation cleanup has completed; cancel the token and await the call.

For live steering, use `Session::prompt_with_steering` with a host-owned bounded `mpsc::Receiver<String>`. `Event::SteeringApplied` acknowledges a committed user message. Steering is injected only after complete tool batches; the receiver retains unconsumed messages when a run ends. Hosts own follow-up scheduling and queue/cancellation policy.

See [`crates/libhabibi/examples/embed.rs`](crates/libhabibi/examples/embed.rs) for a complete executable example. It denies all tools by default:

```sh
cargo run -p libhabibi --example embed -- \
  "$HOME/.config/habibi/auth" /path/to/project \
  'Suggest a project inspection plan without calling tools.'
```

## Validate and measure

Requires Rust 1.89+; tested locally with Rust 1.99.1. The PTY smoke script additionally requires Linux and Python 3, with no third-party Python packages.

```sh
cargo fmt --all -- --check
cargo test --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
cargo build --release -p habibi
python scripts/smoke.py
```

Tests cover SSE chunk boundaries, request/history translation, both providers' native agent loops against local HTTP fixtures, approval and denial, cancellation, shell-descendant cleanup, edits, path escape rejection, credential permissions, and session recovery/locking. The PTY smoke test also verifies that remote callback entry is hidden, invalid state is rejected without echoing the code, and terminal settings are restored on cancellation. Tests do not authenticate real accounts or consume subscription quota.

One local warm-start measurement of the release build:

| Measurement | Result |
| --- | ---: |
| Stripped binary | 4.06 MiB |
| `--help`, median of 20 runs | 0.58 ms |
| TUI first paint, median of 5 runs | 2.38 ms |
| Idle resident memory, median of 5 runs | 5.06 MiB |

These include process launch; first paint includes local session creation. They are machine-specific warm-cache measurements, not cold-start guarantees or a comparison against other agents. No model request is included.

## Deliberately absent

Subagents, repository indexing, API-key providers, and streaming shell output. Add these only when an actual workflow justifies their cost. The public Rust API and journal format are prototype-level and may change.

## CI and releases

GitHub Actions tests and packages all four supported targets, checks Rust 1.89 compatibility, and runs the Linux PTY smoke tests and hermetic installer tests. Stable `v*` tags matching the workspace version trigger a gated release with archives and `SHA256SUMS`. GitHub Pages deploys the static website and canonical installer from `main`. Actions are pinned to commit SHAs and tracked by Dependabot.

See [the release checklist](docs/releasing.md) for publishing, platform limitations, and installer configuration.

### Control a running instance

Every running Habibi instance exposes a private Unix control socket—even the
TUI and plain `-p` runs. The startup output shows its path. Other local processes
can send JSON Lines requests to check status, steer, queue follow-ups, answer
pending approvals, stop, or shut down the instance without taking over its
terminal. See [Local RPC and socket control](docs/rpc.md) for discovery,
permissions, protocol examples, and mode-specific behavior.

### Reading images and other files

`read` displays UTF-8 text with line numbers and attaches PNG, JPEG, GIF, and
WebP files as native image content (detected by signature, not filename).
The selected model must support vision. Ollama's model capability metadata is
checked before sending images; models without reported vision support produce
an actionable error. Hosted providers validate support for the selected model.
Image reads retain normal approvals, workspace confinement, and the 2 MiB file
limit. Line offset/limit apply only to text. Code-mode automatically forwards
images from approved child reads, with an 8 MiB combined base64 limit per call.
Images are retained in session journals; treat those journals as sensitive.
Context accounting reserves 16 KiB of text-equivalent budget per image rather
than counting base64 transport bytes (an estimate, not an exact token count).

PDFs and other binary formats are not automatically decoded. `read` explains
when extraction is needed: for example, use `pdftotext` for a PDF's text or
render its pages to images for visual inspection. These commands require
approval and the relevant local utilities.
