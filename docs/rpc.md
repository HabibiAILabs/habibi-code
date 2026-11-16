# Headless runs and local RPC (experimental, protocol v1)

## One-shot CLI

```sh
habibi -p 'Summarize the architecture of this repository'
habibi --provider ollama --model YOUR_MODEL -p 'Explain src/main.rs'
habibi --resume /path/to/session.jsonl -p 'Continue the investigation'
# Explicitly trust every tool, including unsandboxed shell commands:
habibi -p 'Implement the requested fix and test it' --yolo
```

`-p` / `--prompt` selects headless mode and supplies the initial user message.
The process streams assistant text to stdout and exits when the run ends. Journal
location and tool-denial diagnostics go to stderr; failures produce a nonzero exit.
No terminal is required. Provider/model/workspace options work as in the TUI;
resume restores those values from the journal.

**Saved TUI YOLO preferences do not auto-approve headless tools.** One-shot calls
are denied by default; pass `--yolo` only when explicitly trusting the run, or use
RPC to grant individual approvals. `--yolo` is process-local, not saved as a new
user default. A denied tool does not itself imply a failed run: the model can
recover and return an answer.

Configured, enabled MCP programs still start on the first prompt when MCP is
allowed by the session settings. Those programs are trusted and unsandboxed,
independently of tool-call approvals.

## Persistent RPC

```sh
habibi --rpc
habibi --rpc -p 'Start investigating this bug'
habibi --rpc --resume /path/to/session.jsonl
```

RPC is **JSON Lines over stdin/stdout**, not an HTTP listener or the JSON-RPC 2.0
specification. Spawn a child with pipes and keep stdin open. Stdout contains only
protocol records; diagnostics may appear on stderr. There is no network port,
authentication endpoint, daemon, or attachment to an already-running TUI. The
parent process owns this instance; other local processes can interact through a
parent-provided pipe/broker. Each instance owns one journal and one active run.

Each request occupies one UTF-8 line (at most 1 MiB):

```json
{"id":"request-1","method":"prompt","params":{"text":"Inspect the repository"}}
```

Use unique string or numeric IDs to correlate responses. IDs are correlation
values, **not idempotency keys**: replaying a request can repeat work. A response
is either:

```json
{"id":"request-1","result":{"accepted":true}}
{"id":"request-2","error":{"message":"Agent busy; use steer or follow_up"}}
```

Malformed JSON gets an error with `id: null`. Oversized lines close the transport
and cancel the active run. Unknown methods and invalid parameters return errors
without terminating the server. Responses and events can interleave.

### Methods

| Method | Parameters | Behavior |
|---|---|---|
| `prompt` | `{"text":"..."}` | Start an idle agent. Rejected while busy. |
| `steer` | `{"text":"..."}` | Queue a correction for the active run. Applied at a safe model/tool boundary. Rejected while idle or stopping. |
| `follow_up` | `{"text":"..."}` | Queue another conversation turn; starts after the current run. Starts immediately if idle. |
| `stop` | `{}` | Cancel active work, deny pending approvals, and clear pending follow-ups/steering. The instance stays alive. Wait for `idle` before submitting another prompt. |
| `approve` | `{"call_id":"...","allow":true}` | Answer a pending permission request exactly once. Use `false` to deny. Unknown/expired IDs are errors. |
| `status` | `{}` | Return `session_id`, `busy`, `stopping`, `queued`, and `pending_approvals`. |
| `shutdown` | `{}` | Acknowledge, cancel/drain active work, then exit. |

Text must be nonblank and at most 524288 UTF-8 bytes. The session context budget
can impose a smaller limit when a prompt executes. Follow-up and steering queues
each accept up to 32 entries. On successful completion, unconsumed steering
becomes conversation turns before ordinary follow-ups. Stop, disconnect, or run
failure discards pending queued work rather than executing it unexpectedly.

### Events and lifecycle

The first output record announces readiness:

```json
{"event":"ready","version":1,"session_id":"...","journal":"...","tools":"ask"}
```

`tools` is `auto_approve` only when `--yolo` was explicitly supplied. RPC ignores
saved TUI YOLO for approval policy. `ready` does not mean MCP servers have already
connected: connections are established before the first model request.

Principal events:

- `run_started`
- `text_delta`: `text`
- `permission`: `call` containing `id`, `name`, and `arguments`
- `tool_started`: `call`
- `tool_finished`: `call_id`, `output`, `is_error`
- `steering_applied`: `text`
- `provider_retry`: `retry`, `max_retries`, `delay_secs`
- `context_usage`: `used`, `limit`
- `inspection`: `definitions`, `history` (potentially sensitive tool/context data)
- `compaction_started`; `compacted`: `before`, `after`
- `run_finished`: `cancelled`, `error` (from the agent engine)
- `idle`: `cancelled`, `error`, `queued` (the controller has reclaimed the session)

Use **`idle`** as the completion boundary. A startup/MCP error can produce `idle`
without `run_started`/`run_finished`. `accepted` acknowledges queuing, not successful
execution. A queued follow-up can immediately start after `idle`; consult `queued`
or `status`. Runtime/model errors are reported in events and keep RPC alive.

Every tool requires its own approval, including code-mode child calls. Never
blindly answer `permission` with `true` for untrusted requests. Tool arguments and
output may contain secrets; clients must handle logs accordingly. Treat received
text as untrusted when rendering it to a terminal or browser.

Closing stdin, Ctrl-C, or `shutdown` cancels active work. In-flight local mutations
are awaited before returning; completed side effects are not rolled back. Pending
approvals are denied. Always continuously drain stdout (and stderr) to avoid pipe
backpressure; this is a parent-owned streaming protocol, not a detached job queue.

### Minimal Python client

This example denies tools. Replace the approval branch with an actual user or
policy decision for a tool-enabled integration.

```python
import json
import subprocess

p = subprocess.Popen(
    ["habibi", "--rpc"],
    stdin=subprocess.PIPE,
    stdout=subprocess.PIPE,
    text=True,
    # stderr is inherited so it cannot fill an unread pipe.
)

def send(request_id, method, **params):
    p.stdin.write(json.dumps({"id": request_id, "method": method, "params": params}) + "\n")
    p.stdin.flush()

print(json.loads(p.stdout.readline()))  # ready
send("start", "prompt", text="Explain the repository")
for line in p.stdout:
    message = json.loads(line)
    if message.get("event") == "text_delta":
        print(message["text"], end="", flush=True)
    elif message.get("event") == "permission":
        call_id = message["call"]["id"]
        send("approval-" + call_id, "approve", call_id=call_id, allow=False)
    elif message.get("event") == "idle":
        send("exit", "shutdown")
# Continue draining until EOF; shutdown acknowledges before it finishes cleanup.
p.wait()
```

## Control any running instance (Unix)

Every TUI, `-p`, and `--rpc` instance also opens a private Unix socket, without
requiring `--rpc`. Its path is displayed in the TUI startup transcript or on
headless stderr (and included in the stdio RPC `ready` event).

Discovery: look for `habibi-<pid>-*/instance.json` under `$XDG_RUNTIME_DIR`, or
under the system temporary directory when that variable is unset. The metadata
contains `pid`, `session_id`, `journal`, and `socket`. Directories are created
with mode 0700; sockets have mode 0600. Treat access as full control of the agent:
other processes running as your user can submit work and approve tools.

The socket uses the same newline-delimited JSON requests and `id`-correlated
responses as stdio RPC. It is **request/reply only**, with no event stream.
Supported methods: `status`, `prompt`, `steer`, `follow_up`, `approve`, `stop`,
and `shutdown`. Poll `status` to monitor work; TUI status also includes the
pending tool call as `permission`. TUI tool approvals remain visible locally.
Plain `-p` still denies tools unless `--yolo` is explicit; attaching a socket
client does not change its approval policy. In `--rpc` mode approvals can be
answered over either transport.

```python
import json, socket

with socket.socket(socket.AF_UNIX) as client:
    client.connect("/path/from/startup/control.sock")
    stream = client.makefile("rwb")
    request = {"id": 1, "method": "steer",
               "params": {"text": "Focus on the failing test first."}}
    stream.write(json.dumps(request).encode() + b"\n")
    stream.flush()
    print(json.loads(stream.readline()))
```

Connections can persist across requests. Disconnecting a socket client does not
cancel work or close Habibi. `stop` clears queued work and cancels the current
run; `shutdown` also exits the instance. `-p` remains one-shot: its socket exists
only until its current and queued work finish. Socket requests do not execute
TUI slash commands. There are at most 16 connected clients, 32 buffered requests,
and 1 MiB per request. Normal exit removes the socket and discovery directory;
a forced kill may leave stale entries (a failed connection is not a live agent).
