# Habibi

A small, native Rust coding-agent prototype: **`libhabibi` owns the agent loop; `habibi` is its full-screen terminal frontend.** No JavaScript runtime, daemon, or provider-agent subprocess. A best-effort background GitHub release check is the only idle network activity; it never contacts model providers.

## What's new in v0.2.0

[Release notes and downloads](https://github.com/HabibiAILabs/habibi-code/releases/tag/v0.2.0) · [Quick-start website](https://habibiailabs.github.io/habibi-code/)

- **[Agent Skills](#agent-skills):** automatic frontmatter discovery from `~/.agents` and workspace `.agents`, approval-gated `load_skill`, configurable search directories, and sticky instructions **ON by default**.
- **[Context and tool-call inspector](#context-and-tool-call-inspector):** `/inspect` opens searchable tabs for active context and recorded tool calls, with latest/all scope and history across compaction/resume.
- **[Compaction](#native-core):** `/compact` and automatic summarization at 80% of the byte budget, with original journal records retained.
- **Animated activity:** a bouncing ball in the prompt border shows thinking, responses, tools, and compaction without duplicating the label in the footer.
- **Unlimited turns by default:** `/settings` uses **0 = unlimited**; existing sessions retain their saved turn cap.
- **Session-persistent YOLO:** after initial confirmation, saved YOLO mode is **active immediately on resume**. Disable it in `/settings` if you want per-tool approval again.

### Upgrading from v0.1.0

Run the installer below again, then restart Habibi. Existing v0.1.0 sessions are readable,
but journals updated by v0.2.0 contain new record types: use **v0.2.0 or later** to
resume them. Back up a journal before opening it if you need to retain downgrade access.
Settings remain **per session**, not global preferences. In particular, a saved YOLO
preference from v0.1.0 is now automatically restored as active; shell commands are
still unsandboxed and retain your account's filesystem/network access.

## Install v0.2

Prebuilt Linux x86-64/ARM64 (static musl) and macOS 15+ Intel/Apple Silicon binaries are published on [GitHub Releases](https://github.com/habibiailabs/habibi-code/releases). macOS binaries are unsigned and not notarized.

```sh
curl -fsSL https://habibiailabs.github.io/habibi-code/install.sh | sh
```

The installer verifies SHA-256 checksums and atomically installs to `~/.local/bin` without sudo. Add that directory to PATH if prompted. [Review the installer](scripts/install.sh) before executing remote shell code. Checksums detect corruption; they are not independent publisher signatures.

```sh
# Pin a version or choose an absolute installation directory
curl -fsSL https://habibiailabs.github.io/habibi-code/install.sh | \
  HABIBI_VERSION=v0.2.0 HABIBI_INSTALL_DIR="$HOME/bin" sh
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
target/release/habibi --provider codex --model gpt-5.3-codex --cwd /path/to/project
target/release/habibi --provider claude --model claude-sonnet-4-6
target/release/habibi --resume /path/to/session.jsonl
target/release/habibi logout codex
```

`--resume` restores the saved model, provider, workspace, and history. It cannot be combined with overrides. Model availability depends on your account; use `--model` to override the defaults.

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
- **Ctrl-U:** clear input. **/help**, **/quit**, **/model**, **/settings**, **/compact**, **/inspect**.
- **Activity animation:** while running, the prompt border shows a bouncing ball with **Thinking**, **Responding**, **Executing tool call: <name>**, or **Compacting context**. It pauses for approvals and stops when idle. The footer keeps safety/cancellation hints without repeating the activity label.
- **/inspect:** open a read-only context and tool-call inspector while idle (see below).
- **/model:** edit the current provider’s model ID in a modal; Ctrl-D restores the provider default. Custom IDs are supported; availability depends on your account.
- **/settings:** edit model, maximum model turns per prompt (**0 = unlimited**, the default), context budget in bytes, auto-compaction percentage (default **80%**, **0** disables it), sticky skills, and the skill directory list. Tab/Up/Down selects a field, Ctrl-U clears it, Enter validates and applies, and Esc discards the draft. Settings are session-local and restored by `--resume`; provider and workspace remain fixed. Changes are available only while idle.
- **Status bar:** `/settings` lets you toggle activity, model, RAM, context, and keyboard hints individually using Space or Left/Right. Current-process resident RAM refreshes once a second on Linux (`n/a` elsewhere); child-process memory is not included. Context reports serialized conversation plus skill-context bytes used / configured limit and percentage, **not provider tokens or the actual model context window**. Usage updates as conversation messages are committed, not per streamed token.
- **Tool permissions:** `/settings` selects **Ask every time** (default) or **YOLO** (auto-approve every tool, including reads, edits, writes, and unsandboxed shell commands). Enabling YOLO requires a second Enter to confirm the warning. The preference is saved with the session and **automatically restored on resume**, without another confirmation. New sessions default to Ask. YOLO and active approval/cancellation indicators cannot be hidden by status-bar toggles. Only enable it for trusted work; shell commands retain your account’s filesystem/network access.

Approval previews over 24 KiB are explicitly truncated; do not approve a change you cannot evaluate. The composer is a bounded multiline editor, not a full terminal-editor implementation.

### Context and tool-call inspector

Run **`/inspect`** while idle to open a two-tab modal:

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

- Direct streaming model requests to the ChatGPT Codex backend and Anthropic Messages endpoint.
- OAuth PKCE, callback state checks, refresh-token rotation, and cross-process refresh locking.
- Built-in workspace tools: `read`, `write`, exact single-match `edit`, and `bash`.
- Tools run sequentially. File writes use atomic replacement. Reads/writes are limited to 2 MiB; shell output is captured, drained, and bounded to 64 KiB per stream.
- Cancellation drops HTTP requests and kills shell process groups on Unix, including descendants. Already-completed edits are not undone; in-progress atomic file operations finish before cancellation returns.
- Private append-only JSONL sessions, exclusive session locks, resume, torn-tail recovery, and interrupted-tool-result repair.
- Bounded event channels and terminal transcript. Terminal control characters are removed from displayed output.

Runs have no model-turn limit by default. Set a positive maximum in `/settings` to cap turns per prompt; **0** means unlimited. Existing saved sessions retain their configured limit. The default context limit is 1 MiB of serialized transcript, **not a token estimate**. Before a model turn, the agent automatically summarizes at the configured percentage (80% by default). Use **/compact** while idle to manually summarize, including after `habibi --resume SESSION.jsonl`, then submit your next prompt to continue. Compaction uses the selected provider/model with tools disabled and replaces active history with a continuation summary; original records remain in the journal. Settings and summary checkpoints survive resume. Summary generation is lossy and requires working provider access; a failed/cancelled summary leaves history unchanged. The byte budget is not the provider context window: if the provider already rejects the history as too large, summarization may also fail. With auto-compaction off, exceeding the budget stops the run explicitly.

### Subscription caveat

**These are experimental direct adapters, not verified provider-approved integrations. Live subscription authentication and inference have not yet been tested for this prototype.** Protocol support is based on the direct integrations in Pi 1.5.0; it is not a promise that an account will authorize Habibi.

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
- Sessions: `$XDG_DATA_HOME/habibi/sessions/`, or `~/.local/share/habibi/sessions/`.

`--help` and `--version` do not touch configuration. The idle TUI does not load credentials or contact a provider. Its optional background release check only contacts GitHub (see Update notices above).

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
