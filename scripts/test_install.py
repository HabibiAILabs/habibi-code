#!/usr/bin/env python3
"""Hermetic installer tests: fake HTTPS transport, real tar/checksum/install steps."""
import hashlib
import io
import os
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile

installer = Path(__file__).with_name("install.sh").resolve()
with tempfile.TemporaryDirectory(prefix="habibi-installer-") as directory:
    root = Path(directory)
    fixture = root / "releases"
    fixture.mkdir()
    mock = root / "mock-bin"
    mock.mkdir()
    (mock / "uname").write_text('#!/bin/sh\ncase "$1" in -s) echo "$TEST_OS";; -m) echo "$TEST_ARCH";; *) exit 1;; esac\n')
    (mock / "uname").chmod(0o755)
    (mock / "curl").write_text(f"#!{sys.executable}\n" + '''import os, pathlib, shutil, sys
args = sys.argv[1:]
urls = [a for a in args if a.startswith('https://')]
assert len(urls) == 1 and urls[0].startswith('https://github.com/habibiailabs/habibi-code/')
url = urls[0]
if '-I' in args:
    assert url.endswith('/releases/latest')
    print('https://github.com/habibiailabs/habibi-code/releases/tag/v0.1.0', end='')
else:
    assert '/releases/download/v0.1.0/' in url
    source = pathlib.Path(os.environ['TEST_RELEASES']) / url.rsplit('/',1)[1]
    if not source.exists(): sys.exit(22)
    shutil.copyfile(source, args[args.index('-o')+1])
''')
    (mock / "curl").chmod(0o755)
    checksums = []
    for target in ["x86_64-unknown-linux-musl", "aarch64-unknown-linux-musl", "x86_64-apple-darwin", "aarch64-apple-darwin"]:
        path = fixture / f"habibi-v0.1.0-{target}.tar.gz"
        binary = b'#!/bin/sh\nprintf "habibi 0.1.0\\n"\n'
        with tarfile.open(path, "w:gz") as archive:
            info = tarfile.TarInfo("habibi")
            info.size, info.mode = len(binary), 0o755
            archive.addfile(info, io.BytesIO(binary))
        checksums.append(f"{hashlib.sha256(path.read_bytes()).hexdigest()}  {path.name}\n")
    sums = fixture / "SHA256SUMS"
    original_sums = "".join(checksums)
    sums.write_text(original_sums)
    env = dict(os.environ, PATH=str(mock) + os.pathsep + os.environ["PATH"], HOME=str(root), TEST_RELEASES=str(fixture))
    env.pop("HABIBI_VERSION", None)
    env.pop("HABIBI_INSTALL_DIR", None)

    def run(os_name="Linux", arch="x86_64", ok=True, **overrides):
        install_env = dict(env, TEST_OS=os_name, TEST_ARCH=arch, **overrides)
        result = subprocess.run(["sh", str(installer)], env=install_env, capture_output=True, text=True)
        assert (result.returncode == 0) == ok, result.stdout + result.stderr
        return result

    for os_name, arch in [("Linux", "x86_64"), ("Linux", "aarch64"), ("Darwin", "x86_64"), ("Darwin", "arm64")]:
        install_dir = root / f"bin with spaces-{os_name}-{arch}"
        run(os_name, arch, HABIBI_INSTALL_DIR=str(install_dir))
        assert subprocess.check_output([str(install_dir / "habibi" )], text=True).strip() == "habibi 0.1.0"
    run(HABIBI_VERSION="v0.1.0")
    installed = root / ".local/bin/habibi"
    original_binary = installed.read_bytes()
    run("Windows", ok=False)
    run(arch="mips", ok=False)
    run(ok=False, HABIBI_VERSION="v0.1.0;bad")
    run(ok=False, HABIBI_INSTALL_DIR="relative/path")
    sums.write_text(original_sums.replace(checksums[0][:64], "0" * 64))
    assert "checksum mismatch" in run(ok=False).stderr
    assert installed.read_bytes() == original_binary
    sums.write_text(original_sums + checksums[0])
    assert "duplicate" in run(ok=False).stderr
    assert installed.read_bytes() == original_binary
    sums.write_text("")
    run(ok=False)
print("Installer: all 4 platforms, latest/pinned versions, spaces, checksum rejection, atomic preservation: passed")
