#!/usr/bin/env python3
"""POSIX PTY tests for OSC 7577; no credentials or provider requests."""
import base64
import errno
import fcntl
import hashlib
import os
from pathlib import Path
import pty
import re
import select
import signal
import struct
import subprocess
import sys
import tempfile
import termios
import time

BINARY = Path(sys.argv[1] if len(sys.argv) > 1 else "target/debug/habibi").resolve()
TOKEN = "test-only-resume-token-123456789"
REPORT = re.compile(rb"\x1b\]7577;([^\x1b]+)\x1b\\")


def reports(data):
    result = []
    for body in REPORT.findall(data):
        message, mac = body.rsplit(b":mac=", 1)
        assert hashlib.sha256(message + b":" + TOKEN.encode()).hexdigest().encode() == mac
        fields = dict(part.decode().split("=", 1) for part in message.split(b":"))
        assert abs(time.time() - int(fields["time"])) < 60
        assert TOKEN.encode() not in body
        if "argv" in fields:
            fields["argv"] = [base64.b64decode(arg).decode() for arg in fields["argv"].split(",")]
        result.append(fields)
    return result


class Terminal:
    def __init__(self, env, cwd, args):
        self.master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 100, 0, 0))
        self.process = subprocess.Popen([str(BINARY), *args], stdin=slave, stdout=slave,
                                        stderr=slave, cwd=cwd, env=env, start_new_session=True)
        os.close(slave)
        self.data = bytearray()

    def read(self):
        if select.select([self.master], [], [], 0.05)[0]:
            try:
                self.data.extend(os.read(self.master, 65536))
            except OSError as error:
                if error.errno != errno.EIO:
                    raise

    def wait(self, predicate):
        deadline = time.monotonic() + 10
        while not predicate():
            assert time.monotonic() < deadline, f"Timed out waiting for TUI (exit={self.process.poll()}): {bytes(self.data)!r}"
            self.read()

    def close(self, sig=None, keys=b"/quit\r"):
        if sig:
            self.process.send_signal(sig)
        else:
            os.write(self.master, keys)
        self.wait(lambda: self.process.poll() is not None)
        for _ in range(3):
            self.read()
        os.close(self.master)


# Keep Unix socket paths below macOS SUN_LEN even when TMPDIR is deeply nested.
with tempfile.TemporaryDirectory(prefix="habibi-resume-", dir="/tmp") as temp:
    root = Path(temp)
    workspace = root / "space ' $(touch BAD); project"
    workspace.mkdir()
    (root / "runtime").mkdir(mode=0o700)
    env = {**os.environ, "HOME": str(root), "XDG_CONFIG_HOME": str(root / "config"),
           "XDG_DATA_HOME": str(root / "data"), "XDG_CACHE_HOME": str(root / "cache"),
           "XDG_RUNTIME_DIR": str(root / "runtime"), "OSC7577_TOKEN": TOKEN,
           "HABIBI_NO_UPDATE_CHECK": "1", "HABIBI_NO_PROGRAM_STATUS": "1"}
    env.pop("HABIBI_NO_PROGRAM_RESUME", None)
    terminals = []
    try:
        terminal = Terminal(env, workspace, ["--provider", "ollama"])
        terminals.append(terminal)
        terminal.wait(lambda: b"Prompt" in terminal.data and len(reports(terminal.data)) == 1)
        report = reports(terminal.data)[0]
        assert int(report["pid"]) == terminal.process.pid
        argv = report["argv"]
        assert argv[:4] == ["/bin/sh", "-c", 'cd "$1" && exec habibi --resume "$2"', "habibi-resume"]
        assert Path(argv[4]) == workspace.resolve()
        journal = Path(argv[5])
        assert journal.is_absolute() and journal.is_file()
        assert TOKEN not in journal.read_text()
        terminal.process.send_signal(signal.SIGCONT)
        terminal.wait(lambda: len(reports(terminal.data)) == 2)
        terminal.close()
        assert reports(terminal.data)[-1].get("clear") == "1"

        # Exercise the reported wrapper, with a fake habibi that records arguments.
        mock = root / "bin"
        mock.mkdir()
        script = mock / "habibi"
        script.write_text('#!/bin/sh\nprintf "%s\\n" "$PWD" "$1" "$2"\n')
        script.chmod(0o755)
        output = subprocess.check_output(argv, cwd=root, env={**env, "PATH": str(mock)}, text=True)
        assert output.splitlines() == [str(workspace.resolve()), "--resume", str(journal)]
        assert not (root / "BAD").exists()
        assert not (workspace / "BAD").exists()

        for sig in (signal.SIGTERM, signal.SIGHUP):
            resumed = Terminal(env, root, ["--resume", str(journal)])
            terminals.append(resumed)
            resumed.wait(lambda: b"Prompt" in resumed.data and bool(reports(resumed.data)))
            assert reports(resumed.data)[0]["argv"] == argv
            resumed.close(sig=sig)
            assert all("clear" not in r for r in reports(resumed.data))

        for changes in ({"HABIBI_NO_PROGRAM_RESUME": "1"}, {"OSC7577_TOKEN": "invalid"}):
            disabled = Terminal({**env, **changes}, workspace, ["--provider", "ollama"])
            terminals.append(disabled)
            disabled.wait(lambda: b"Prompt" in disabled.data)
            disabled.close(keys=b"\x03")
            assert not REPORT.search(disabled.data)
        for args in (["--help"], ["--version"], ["--rpc", "--provider", "ollama"]):
            output = subprocess.run([str(BINARY), *args], input=b"", env=env, cwd=workspace,
                                    stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=10)
            assert b"\x1b]7577;" not in output.stdout + output.stderr
            assert TOKEN.encode() not in output.stdout + output.stderr
    finally:
        for terminal in terminals:
            if terminal.process.poll() is None:
                terminal.process.kill()
                terminal.process.wait()
                os.close(terminal.master)
print("Program resume: signed reports, safe cd wrapper, resume, SIGCONT, quit, signals, opt-out and non-TUI modes passed")
