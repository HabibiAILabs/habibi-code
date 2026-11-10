# Habibi

A small, native Rust coding-agent prototype: **`libhabibi` owns the agent loop; `habibi` is its full-screen terminal frontend.** No JavaScript runtime, daemon, or provider-agent subprocess. A best-effort background GitHub release check is the only idle network activity; it never contacts model providers.

## Install v0.1

Prebuilt Linux x86-64/ARM64 (static musl) and macOS 15+ Intel/Apple Silicon binaries are published on [GitHub Releases](https://github.com/habibiailabs/habibi-code/releases). macOS binaries are unsigned and not notarized.

```sh
curl -fsSL https://habibiailabs.github.io/habibi-code/install.sh | sh
```

The installer verifies SHA-256 checksums and atomically installs to `~/.local/bin` without sudo. Add that directory to PATH if prompted. [Review the installer](scripts/install.sh) before executing remote shell code. Checksums detect corruption; they are not independent publisher signatures.

```sh
# Pin a version or choose an absolute installation directory
curl -fsSL https://habibiailabs.github.io/habibi-code/install.sh | \
  HABIBI_VERSION=v0.1.0 HABIBI_INSTALL_DIR="$HOME/bin" sh
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

### TUI

- **Enter:** submit while idle; during a run, queue **steering** for the next safe model boundary (after the current response and its tool batch). Steering does not interrupt an in-flight HTTP request or tool.
- **Alt+Enter:** queue a **follow-up**, which starts a separate run when the current run finishes successfully. Steering takes priority over follow-ups. Both queues are bounded to 32 messages; a full queue leaves your draft intact. Queued-but-unapplied messages are in memory only; applied steering and started follow-ups are saved in conversation history.
- **Ctrl+Enter:** insert a newline. **Ctrl-J** is an alternate newline binding. Ctrl+Enter requires a terminal supporting enhanced keyboard reporting; on older terminals that send it as plain Enter, use Ctrl-J or paste multiline text. Input wraps, grows up to six visible rows, and scrolls to the cursor; Left/Right, Home/End, Delete, and Backspace edit the draft. Multiline paste preserves line breaks.
- **Ctrl-Y / Ctrl-N:** approve one proposed tool call or deny it. In the default Ask mode, every tool requires approval, including reads. **F2** opens the scrollable argument inspector, where **y/n** also approve/deny. Plain y/n in the composer is ordinary text, so drafting a message cannot accidentally approve a tool.
- **Esc:** interrupt the active run, including a pending approval, and clear queued messages. In settings it closes the dialog without saving. Already-completed tool effects are not undone. Errors also clear pending queues rather than automatically continuing.
- **Ctrl-C:** cancel an active run; quit when idle.
- **PgUp / PgDn or mouse wheel:** scroll the transcript or open argument inspector. Scrolling up keeps your reading position stable as output arrives; scroll back to the bottom to follow output again. Conversation roles, headings, bold text, inline code, and fenced code blocks have lightweight styling. Tools/results use compact summaries, with full content retained in the session journal. Thinking is shown as a compact activity indicator, never raw private reasoning.
- **Ctrl-U:** clear input. **/help**, **/quit**, **/model**, **/settings**.
- **/model:** edit the current provider’s model ID in a modal; Ctrl-D restores the provider default. Custom IDs are supported; availability depends on your account.
- **/settings:** edit model, maximum model turns per prompt, and context budget in bytes. Tab/Up/Down selects a field, Ctrl-U clears it, Enter validates and applies, and Esc discards the draft. Settings are session-local and restored by `--resume`; provider and workspace remain fixed. Changes are available only while idle.
- **Status bar:** `/settings` lets you toggle activity, model, RAM, context, and keyboard hints individually using Space or Left/Right. Current-process resident RAM refreshes once a second on Linux (`n/a` elsewhere); child-process memory is not included. Context reports serialized conversation bytes used / configured limit and percentage, **not provider tokens or the actual model context window**. Usage updates as conversation messages are committed, not per streamed token.
- **Tool permissions:** `/settings` selects **Ask every time** (default) or **YOLO** (auto-approve every tool, including reads, edits, writes, and unsandboxed shell commands). Enabling YOLO requires a second Enter to confirm the warning. The preference is saved, but after a restart/resume YOLO stays **inactive** until you confirm it again in `/settings`. YOLO and active approval/cancellation indicators cannot be hidden by status-bar toggles. Only enable it for trusted work; shell commands retain your account’s filesystem/network access.

Approval previews over 24 KiB are explicitly truncated; do not approve a change you cannot evaluate. The composer is a bounded multiline editor, not a full terminal-editor implementation.

## Native core

- Direct streaming model requests to the ChatGPT Codex backend and Anthropic Messages endpoint.
- OAuth PKCE, callback state checks, refresh-token rotation, and cross-process refresh locking.
- Four built-in tools: `read`, `write`, exact single-match `edit`, and `bash`.
- Tools run sequentially. File writes use atomic replacement. Reads/writes are limited to 2 MiB; shell output is captured, drained, and bounded to 64 KiB per stream.
- Cancellation drops HTTP requests and kills shell process groups on Unix, including descendants. Already-completed edits are not undone; in-progress atomic file operations finish before cancellation returns.
- Private append-only JSONL sessions, exclusive session locks, resume, torn-tail recovery, and interrupted-tool-result repair.
- Bounded event channels and terminal transcript. Terminal control characters are removed from displayed output.

The default run limit is 32 model turns per prompt. The default context limit is 1 MiB of serialized transcript, **not a token estimate**. The agent stops explicitly at that limit; it does not silently discard history.

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

Plugins, embedded scripting, MCP, subagents, repository indexing, automatic compaction/retries, custom tool registration, API-key providers, and streaming shell output. Add these only when an actual workflow justifies their cost. The public Rust API and journal format are prototype-level and may change.

## CI and releases

GitHub Actions tests and packages all four supported targets, checks Rust 1.89 compatibility, and runs the Linux PTY smoke tests and hermetic installer tests. Stable `v*` tags matching the workspace version trigger a gated release with archives and `SHA256SUMS`. GitHub Pages deploys the static website and canonical installer from `main`. Actions are pinned to commit SHAs and tracked by Dependabot.

See [the release checklist](docs/releasing.md) for publishing, platform limitations, and installer configuration.
