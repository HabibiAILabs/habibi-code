# Habibi

A small, native Rust coding-agent prototype: **`libhabibi` owns the agent loop; `habibi` is its full-screen terminal frontend.** No JavaScript runtime, Habibi daemon, or provider-agent subprocess (Ollama runs as a separate server). A best-effort background GitHub release check is the only idle network activity; it never contacts model providers.

## What's new in v0.3.1

[Release notes](RELEASE_NOTES.md) · [Quick-start website](https://habibiailabs.github.io/habibi-code/)

- **Commands during execution:** `/inspect`, `/settings`, `/model`, `/models`, `/help`, and `/update` work without interrupting the agent. Inspection shows the latest context-boundary snapshot; close and reopen to refresh.
- **Slash-command suggestions:** type `/` to see matches; press Tab to complete and cycle them.
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

`--resume` restores the saved model, provider, workspace, and history. It cannot be combined with overrides. Model availability depends on your account; use `--model` to override the defaults.

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
- **Ctrl-C:** cancel an active run; quit when idle.
- **PgUp / PgDn or mouse wheel:** scroll the transcript or open argument inspector. Scrolling up keeps your reading position stable as output arrives; scroll back to the bottom to follow output again. Conversation roles, headings, bold text, inline code, and fenced code blocks have lightweight styling. Tools/results use compact summaries, with full content retained in the session journal. Thinking is shown as a compact activity indicator, never raw private reasoning.
- **Tab:** complete slash commands; press again to cycle matches. Suggestions appear as you type `/`.
- **Ctrl-U:** clear input. **/help**, **/quit**, **/models**, **/model**, **/settings**, **/compact**, **/inspect**.
- **Activity animation:** while running, the prompt border shows a bouncing ball with **Thinking**, **Responding**, **Executing tool call: <name>**, or **Compacting context**. It pauses for approvals and stops when idle. The footer keeps safety/cancellation hints without repeating the activity label.
- **/inspect:** open a read-only context and tool-call inspector, including during execution (see below).
- **/models:** list bundled subscription choices or discover installed Ollama models.
- **/model:** choose with Ctrl-N/P or type a model ID in a modal; Ctrl-D restores the bundled provider default. Enter saves a default for future new sessions. Custom IDs are supported; availability depends on your account/server.
- **/settings:** edit model, maximum model turns per prompt (**0 = unlimited**, the default), context budget in bytes, auto-compaction percentage (default **80%**, **0** disables it), sticky skills, and the skill directory list. Tab/Up/Down selects a field, Ctrl-U clears it, Enter validates and applies, and Esc discards the draft. Model and frontend preferences also become user defaults; other settings remain session-local. `--resume` restores journaled settings; provider and workspace remain fixed. Changes are available only while idle.
- **Status bar:** `/settings` lets you toggle activity, model, RAM, context, and keyboard hints individually using Space or Left/Right. Current-process resident RAM refreshes once a second on Linux (`n/a` elsewhere); child-process memory is not included. Context reports serialized conversation plus skill-context bytes used / configured limit and percentage, **not provider tokens or the actual model context window**. Usage updates as conversation messages are committed, not per streamed token.
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

## Native core

- Direct streaming model requests to the ChatGPT Codex backend, Anthropic Messages endpoint, and native Ollama chat endpoint.
- OAuth PKCE, callback state checks, refresh-token rotation, and cross-process refresh locking.
- Built-in workspace tools: `read`, `write`, exact single-match `edit`, and `bash`.
- Tools run sequentially. File writes use atomic replacement. Reads/writes are limited to 2 MiB; shell output is captured, drained, and bounded to 64 KiB per stream.
- Cancellation drops HTTP requests and kills shell process groups on Unix, including descendants. Already-completed edits are not undone; in-progress atomic file operations finish before cancellation returns.
- Private append-only JSONL sessions, exclusive session locks, resume, torn-tail recovery, and interrupted-tool-result repair.
- Bounded event channels and terminal transcript. Terminal control characters are removed from displayed output.

Runs have no model-turn limit by default. Set a positive maximum in `/settings` to cap turns per prompt; **0** means unlimited. Existing saved sessions retain their configured limit. The default context limit is 1 MiB of serialized transcript, **not a token estimate**. Before a model turn, the agent automatically summarizes at the configured percentage (80% by default). Use **/compact** while idle to manually summarize, including after `habibi --resume SESSION.jsonl`, then submit your next prompt to continue. Compaction uses the selected provider/model with tools disabled and replaces active history with a continuation summary; original records remain in the journal. Settings and summary checkpoints survive resume. Summary generation is lossy and requires working provider access; a failed/cancelled summary leaves history unchanged. The byte budget is not the provider context window: if the provider already rejects the history as too large, summarization may also fail. With auto-compaction off, exceeding the budget stops the run explicitly.

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

Plugins, embedded scripting, MCP, subagents, repository indexing, automatic retries, custom tool registration, API-key providers, and streaming shell output. Add these only when an actual workflow justifies their cost. The public Rust API and journal format are prototype-level and may change.

## CI and releases

GitHub Actions tests and packages all four supported targets, checks Rust 1.89 compatibility, and runs the Linux PTY smoke tests and hermetic installer tests. Stable `v*` tags matching the workspace version trigger a gated release with archives and `SHA256SUMS`. GitHub Pages deploys the static website and canonical installer from `main`. Actions are pinned to commit SHAs and tracked by Dependabot.

See [the release checklist](docs/releasing.md) for publishing, platform limitations, and installer configuration.
