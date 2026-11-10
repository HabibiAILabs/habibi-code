#!/usr/bin/env python3
"""Linux PTY smoke test and warm-start measurements; no accounts or network needed.

Usage: cargo build --release -p habibi && python scripts/smoke.py
"""
import fcntl
import os
from pathlib import Path
import pty
import select
import statistics
import struct
import subprocess
import sys
import tempfile
import termios
import time

binary = Path(sys.argv[1] if len(sys.argv) > 1 else "target/release/habibi").resolve()


def launch(args, env, workspace):
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 100, 0, 0))
    initial = termios.tcgetattr(slave)
    started = time.perf_counter()
    process = subprocess.Popen(
        [str(binary), *args], stdin=slave, stdout=slave, stderr=slave,
        cwd=workspace, env=env, start_new_session=True,
    )
    output = bytearray()
    try:
        deadline = time.monotonic() + 5
        while b"Prompt" not in output:
            assert time.monotonic() < deadline, "TUI first paint timed out: " + repr(output)
            if select.select([master], [], [], 0.1)[0]:
                output.extend(os.read(master, 65536))
            assert process.poll() is None, "TUI exited before first paint: " + repr(output)
        first_paint = (time.perf_counter() - started) * 1000
        # Linux current resident memory, not peak memory. The process is idle here.
        status = Path(f"/proc/{process.pid}/status").read_text()
        rss = next(int(line.split()[1]) for line in status.splitlines() if line.startswith("VmRSS:"))
        def send_and_wait(keys, marker):
            os.write(master, keys)
            received = bytearray()
            deadline = time.monotonic() + 5
            while marker not in received:
                assert time.monotonic() < deadline, "Modal interaction timed out: " + repr(received)
                if select.select([master], [], [], 0.1)[0]:
                    received.extend(os.read(master, 65536))
                assert process.poll() is None, "TUI exited during modal interaction"
            output.extend(received)

        send_and_wait(b"/model\r", b"Model ID")
        send_and_wait(b"\x15smoke-custom-model\r", b"saved.")
        send_and_wait(b"/settings\r", b"Max model turns")
        # Invalid numeric drafts stay open; Esc discards them.
        send_and_wait(b"\t\x15x\r", b"integer")
        os.write(master, b"\x1b")
        time.sleep(0.1)
        send_and_wait(b"/settings\r", b"Tool permissions")
        send_and_wait(b"\t\t\t \r", b"confirm.")
        send_and_wait(b"\r", b"saved.")
        send_and_wait(b"/settings\r", b"Tool permissions")
        send_and_wait(b"\t\t\t \r", b"saved.")
        # Enhanced keyboard Ctrl+Enter must insert a newline, not submit a run.
        send_and_wait(b"first-line\x1b[13;5usecond-line", b"second-line")
        os.write(master, b"\x15")
        send_and_wait(b"\x1b[200~pasted-first\npasted-second\x1b[201~", b"second")
        os.write(master, b"\x15")
        os.write(master, b"/help\r")
        time.sleep(0.05)
        os.write(master, b"\x03")
        # Drain during shutdown so terminal output cannot fill the PTY and block exit.
        deadline = time.monotonic() + 5
        while process.poll() is None:
            assert time.monotonic() < deadline, "TUI shutdown timed out"
            if select.select([master], [], [], 0.05)[0]:
                output.extend(os.read(master, 65536))
        assert process.returncode == 0, repr(output)
        restored = termios.tcgetattr(slave)
        assert restored[3] & (termios.ICANON | termios.ECHO) == initial[3] & (termios.ICANON | termios.ECHO), "terminal mode was not restored"
        assert b"\x1b[?1049l" in output, "alternate screen was not restored"
        assert b"Ctrl-U" in output, "/help did not render"
        return first_paint, rss
    finally:
        if process.poll() is None:
            process.kill()
            process.wait()
        os.close(master)
        os.close(slave)


def check_private_login(env, workspace, cancel=False):
    master, slave = pty.openpty()
    initial = termios.tcgetattr(slave)
    process = subprocess.Popen(
        [str(binary), "login", "claude"], stdin=slave, stdout=slave, stderr=slave,
        cwd=workspace, env=env, start_new_session=True,
    )
    output = bytearray()
    marker = b"fake-private-code-do-not-echo"
    try:
        deadline = time.monotonic() + 5
        while b"Input hidden:" not in output:
            assert time.monotonic() < deadline, "private login prompt timed out"
            if select.select([master], [], [], 0.1)[0]:
                output.extend(os.read(master, 65536))
            assert process.poll() is None, "login exited before input"
        assert not termios.tcgetattr(slave)[3] & termios.ECHO, "callback input echoes"
        if cancel:
            os.write(master, b"\x03")
        else:
            # Invalid state must fail before token exchange; no real account/network used.
            os.write(master, b"\x1b[200~" + marker + b"#wrong-state\x1b[201~\r")
        deadline = time.monotonic() + 5
        while process.poll() is None:
            assert time.monotonic() < deadline, "private login shutdown timed out"
            if select.select([master], [], [], 0.05)[0]:
                output.extend(os.read(master, 65536))
        while select.select([master], [], [], 0)[0]:
            output.extend(os.read(master, 65536))
        assert process.returncode != 0
        assert marker not in output, "private callback was echoed or logged"
        assert (b"Login cancelled" if cancel else b"OAuth state mismatch") in output, repr(output)
        restored = termios.tcgetattr(slave)
        assert restored[3] & (termios.ICANON | termios.ECHO) == initial[3] & (termios.ICANON | termios.ECHO), "login did not restore terminal"
    finally:
        if process.poll() is None:
            process.kill()
            process.wait()
        os.close(master)
        os.close(slave)


with tempfile.TemporaryDirectory(prefix="habibi-smoke-") as directory:
    root = Path(directory)
    env = dict(os.environ, HABIBI_NO_UPDATE_CHECK="1", TERM="xterm-256color", XDG_CONFIG_HOME=str(root / "config"), XDG_DATA_HOME=str(root / "data"))
    timings = []
    for _ in range(20):
        started = time.perf_counter()
        result = subprocess.run([str(binary), "--help"], env=env, capture_output=True, check=True)
        timings.append((time.perf_counter() - started) * 1000)
        assert b"habibi" in result.stdout
    assert not (root / "config").exists() and not (root / "data").exists(), "--help touched configuration"
    paints, memory = [], []
    for _ in range(5):
        paint, rss = launch(["--cwd", str(root)], env, root)
        paints.append(paint)
        memory.append(rss)
    sessions = list((root / "data/habibi/sessions").glob("*.jsonl"))
    assert len(sessions) == 5
    launch(["--resume", str(sessions[0])], env, root)
    assert not (root / "config").exists(), "idle TUI accessed credentials"
    check_private_login(env, root)
    check_private_login(env, root, cancel=True)
    assert not (root / "config").exists(), "failed/cancelled login wrote credentials"
    print("PTY launch, model/settings modals, validation, YOLO confirmation, multiline input/paste, /help, terminal restoration, session persistence and resume: passed")
    print("Remote login paste: hidden input, state rejection, cancellation and terminal restoration passed")
    print(f"Binary: {binary.stat().st_size / 1024 / 1024:.2f} MiB")
    print(f"Warm --help median (20 runs): {statistics.median(timings):.2f} ms")
    print(f"Warm TUI first-paint median (5 runs): {statistics.median(paints):.2f} ms")
    print(f"Idle RSS median (5 runs): {statistics.median(memory) / 1024:.2f} MiB")
