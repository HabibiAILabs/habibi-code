#!/bin/sh
# Installs a checksum-verified release. Requires curl, tar, and sha256sum or shasum.
set -eu
fail() { printf 'habibi: %s\n' "$*" >&2; exit 1; }
command -v curl >/dev/null 2>&1 || fail 'curl is required'
command -v tar >/dev/null 2>&1 || fail 'tar is required'
if command -v sha256sum >/dev/null 2>&1; then hash_command=sha256sum
elif command -v shasum >/dev/null 2>&1; then hash_command=shasum
else fail 'sha256sum or shasum is required'; fi

case "$(uname -s)" in
    Linux) platform=unknown-linux-musl ;;
    Darwin) platform=apple-darwin ;;
    *) fail 'supported systems: Linux and macOS' ;;
esac
case "$(uname -m)" in
    x86_64|amd64) arch=x86_64 ;;
    arm64|aarch64) arch=aarch64 ;;
    *) fail 'supported architectures: x86-64 and ARM64' ;;
esac
repo=https://github.com/habibiailabs/habibi-code
fetch() { curl --proto '=https' --tlsv1.2 --connect-timeout 10 --max-time 120 --retry 2 -fsSL "$@"; }
version=${HABIBI_VERSION:-}
if [ -z "$version" ]; then
    url=$(fetch -I -o /dev/null -w '%{url_effective}' "$repo/releases/latest") || fail 'cannot find the latest release'
    version=${url##*/}
fi
printf '%s\n' "$version" | grep -Eq '^v[0-9]+\.[0-9]+\.[0-9]+$' || fail 'HABIBI_VERSION must be a stable tag such as v0.1.0'
archive="habibi-$version-$arch-$platform.tar.gz"
base="$repo/releases/download/$version"
install_dir=${HABIBI_INSTALL_DIR:-${HOME:?HOME is required}/.local/bin}
case "$install_dir" in /*) ;; *) fail 'HABIBI_INSTALL_DIR must be an absolute path' ;; esac
work=$(mktemp -d) || fail 'cannot create temporary directory'
staged=
cleanup() { rm -rf "$work"; if [ -n "$staged" ]; then rm -f "$staged"; fi; }
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM HUP
printf 'Downloading Habibi %s for %s-%s…\n' "$version" "$arch" "$platform"
fetch "$base/$archive" -o "$work/archive.tar.gz" || fail 'release download failed; this platform/version may not be published yet'
fetch "$base/SHA256SUMS" -o "$work/SHA256SUMS" || fail 'checksum download failed'
expected=$(awk -v file="$archive" '$2 == file { print $1; count++ } END { if (count != 1) exit 1 }' "$work/SHA256SUMS") || fail 'missing or duplicate archive checksum'
printf '%s\n' "$expected" | grep -Eq '^[a-f0-9]{64}$' || fail 'invalid checksum format'
if [ "$hash_command" = sha256sum ]; then actual=$(sha256sum "$work/archive.tar.gz" | awk '{print $1}')
else actual=$(shasum -a 256 "$work/archive.tar.gz" | awk '{print $1}'); fi
[ "$actual" = "$expected" ] || fail 'checksum mismatch; nothing installed'
tar -xzf "$work/archive.tar.gz" -C "$work" habibi || fail 'invalid release archive'
[ -f "$work/habibi" ] && [ ! -L "$work/habibi" ] || fail 'release does not contain a regular habibi binary'
mkdir -p "$install_dir" || fail 'cannot create install directory'
staged=$(mktemp "$install_dir/.habibi.XXXXXX") || fail 'cannot stage installation'
cat "$work/habibi" > "$staged"
chmod 755 "$staged"
mv -f "$staged" "$install_dir/habibi"
staged=
printf 'Installed %s/habibi (%s).\n' "$install_dir" "$version"
case ":${PATH:-}:" in *":$install_dir:"*) ;; *) printf 'Add to your shell profile: export PATH="%s:$PATH"\n' "$install_dir" ;; esac
printf 'Next: habibi login codex   # or: habibi login claude\n'
