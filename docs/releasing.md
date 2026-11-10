# Release checklist

1. Update `[workspace.package].version` in `Cargo.toml`, regenerate `Cargo.lock`, and update `RELEASE_NOTES.md` and the website version.
2. Push/merge into `main`. Wait for **CI** to pass on Linux x86-64/ARM64, macOS Intel/Apple Silicon, and the declared minimum Rust version.
3. Tag that tested commit and push the tag:

   ```sh
   git tag -a v0.2.0 -m 'Habibi v0.2.0'
   git push origin v0.2.0
   ```

4. The **Release** workflow rejects mismatched/non-stable version tags, rebuilds and tests all four platforms, and gathers archives plus `SHA256SUMS`. It creates a draft and publishes it only after all assets are uploaded. A rerun can repair an unpublished draft; it refuses to modify an already published release.
5. Verify the latest release and install the actual published archive into a temporary directory before announcing it. Test on physical Linux/macOS machines when available. CI does not validate live provider subscriptions.
6. **Pages** deploys `docs/` plus the canonical `scripts/install.sh` on changes to `main`. Set repository Settings → Pages → Source to **GitHub Actions**. No separate branch is needed.

The tag must be pushed by a maintainer credential, not a workflow's `GITHUB_TOKEN`, so it triggers the release workflow. No PAT is needed by the workflows themselves. Build jobs use read-only tokens; only the publish job has `contents: write`. Pages deployment uses GitHub's OIDC token.

## Assets

- `habibi-vVERSION-x86_64-unknown-linux-musl.tar.gz`
- `habibi-vVERSION-aarch64-unknown-linux-musl.tar.gz`
- `habibi-vVERSION-x86_64-apple-darwin.tar.gz`
- `habibi-vVERSION-aarch64-apple-darwin.tar.gz`
- `SHA256SUMS`
- `install.sh`

Linux builds use musl. macOS builds use the runner's SDK/default deployment target; macOS 15+ is the initial supported baseline. They are unsigned and not notarized; signing/notarization would require separate Apple credentials and a release policy. Checksums share the release's trust boundary and are not cryptographic publisher signatures.

## Installer configuration

```sh
# Default: latest stable, ~/.local/bin, without sudo
curl -fsSL https://habibiailabs.github.io/habibi-code/install.sh | sh

# Pinned version / custom directory (variables belong on sh's side of the pipe)
curl -fsSL https://habibiailabs.github.io/habibi-code/install.sh | \
  HABIBI_VERSION=v0.2.0 HABIBI_INSTALL_DIR="$HOME/bin" sh
```

For review-first installation, download the script to a file, inspect it, then execute it. The installer resolves the latest tag once and pins both the archive and checksum downloads to that tag. A checksum failure leaves an existing installation untouched. No automatic self-update, privilege escalation, shell-profile edits, or telemetry is performed.
