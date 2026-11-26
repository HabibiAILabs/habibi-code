# Release checklist

1. Update `[workspace.package].version` in `Cargo.toml`, regenerate `Cargo.lock`, and update `RELEASE_NOTES.md` and the website version.
2. Push/merge into `main`. Wait for **CI** to pass on Linux x86-64/ARM64, macOS Intel/Apple Silicon, and the declared minimum Rust version.
3. Tag that tested commit and push the tag:

   ```sh
   git tag -a v0.8.0 -m 'Habibi v0.8.0'
   git push origin v0.8.0
   ```

4. The **Release** workflow rejects mismatched/non-stable version tags and selects a successful `ci.yml` run for the **exact tagged commit**. Only `push` or manual runs on `main` in this repository qualify; PR and fork runs never qualify. It downloads the four validated artifact IDs, checks their digests, verifies versioned archive names and contents, then generates `SHA256SUMS`. It does **not** rebuild or request macOS runners. It creates a draft and publishes it only after all assets are uploaded. A rerun can repair an unpublished draft; it refuses to modify an already published release.
5. Verify the latest release and install the actual published archive into a temporary directory before announcing it. Test on physical Linux/macOS machines when available. CI does not validate live provider subscriptions.
6. **Pages** deploys `docs/` plus the canonical `scripts/install.sh` on changes to `main`. Set repository Settings → Pages → Source to **GitHub Actions**. No separate branch is needed.

The tag must be pushed by a maintainer credential, not a workflow's `GITHUB_TOKEN`, so it triggers the release workflow. No PAT is needed by the workflows themselves. Build jobs use read-only tokens; only the publish job has `contents: write`. Pages deployment uses GitHub's OIDC token.

## Build caching and artifact recovery

The four-platform CI remains unchanged in coverage: tests, linting, installer checks,
release builds, and Linux smoke tests still run before publication. Rust dependencies
and build outputs are cached with separate target keys and automatic toolchain/lockfile
keys. Only non-PR runs on `main` save caches. Caching reduces compilation work, not
GitHub's hosted-runner queues. The first build for a new cache is still cold.

CI archives are retained for **30 days**. Release fails closed if no qualifying run
has all four unexpired archives. There is no fallback to a different commit or a
partial release. To rebuild explicitly:

1. Find the CI run for the exact tagged commit and rerun **all jobs**:
   `gh run rerun CI_RUN_ID`. This preserves the original commit and trusted event.
2. Wait for that CI run to succeed, then rerun the failed Release workflow:
   `gh run rerun RELEASE_RUN_ID`.
3. If the tagged commit is still the tip of `main`, an alternative is
   `gh workflow run ci.yml --ref main`; verify its commit matches the tag exactly.
   A manual run on a tag or another branch does not qualify.

Do not move an existing published tag. If the original CI run is no longer available
and `main` has moved on, prepare a new patch release from `main` instead.

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
  HABIBI_VERSION=v0.8.0 HABIBI_INSTALL_DIR="$HOME/bin" sh
```

For review-first installation, download the script to a file, inspect it, then execute it. The installer resolves the latest tag once and pins both the archive and checksum downloads to that tag. A checksum failure leaves an existing installation untouched. No automatic self-update, privilege escalation, shell-profile edits, or telemetry is performed.

## Hosted-runner capacity failures

A job with no steps and “The job was not acquired by Runner of type hosted” is
a runner-capacity failure, not a compiler/test failure. Check the job annotations;
retry failed jobs with `gh run rerun RUN_ID --failed` once capacity is available.
Do not bypass platform checks or publish partial assets. The original v0.5.0
attempt hit this failure on `macos-15` (Apple Silicon); all other platforms passed.
An unpublished tag may be replaced only deliberately, after verifying no public
release exists; published release tags must not be moved.
