# Install a Novum Agent release

These instructions are for an agent installing the prebuilt fork on another
macOS Apple Silicon machine (`Darwin`, `arm64`). Release `novum-v0.1.0` was built
and tested on macOS 15.7.4. It contains both `codex` and `codex-code-mode-host`,
plus the package metadata, ripgrep, and patched zsh they need.

Use the complete package. The CLI and Code Mode host must come from the same
release. Do not copy a lone binary into `PATH` or create the install symlinks by
hand. The commands below use the repository's supported installer and do not
compile Rust or download V8.

## Agent installation steps

Prerequisites: `git`, authenticated `gh`, `just`, Python 3.11 or later, `tar`,
and `shasum`. The target prefix must be writable. The example uses
`/opt/homebrew`, matching the build machine; set `install_prefix="$HOME/.local"`
for a user-owned installation instead.

Run this block in Bash. Keep the temporary directory until verification passes.

```bash
set -euo pipefail
test "$(uname -s)/$(uname -m)" = "Darwin/arm64"

release_version=0.1.0
release_tag="novum-v${release_version}"
install_prefix=/opt/homebrew
install_work=$(mktemp -d "${TMPDIR:-/tmp}/novum-install.XXXXXX")

gh release download "$release_tag" --repo abrookins/novum-agent \
  --pattern 'novum-agent-aarch64-apple-darwin.tar.gz' \
  --pattern SHA256SUMS --pattern BUILD_INFO.json --dir "$install_work"

cd "$install_work"
shasum -a 256 -c SHA256SUMS
mkdir package
tar -xzf novum-agent-aarch64-apple-darwin.tar.gz -C package

git clone --depth 1 --branch "$release_tag" \
  https://github.com/abrookins/novum-agent.git source
cd source

PREFIX="$install_prefix" ./scripts/install-local-codex.sh \
  --package-version "$release_version" \
  --entrypoint-bin "$install_work/package/bin/codex" \
  --code-mode-host-bin "$install_work/package/bin/codex-code-mode-host" \
  --rg-bin "$install_work/package/codex-path/rg" \
  --zsh-bin "$install_work/package/codex-resources/zsh/bin/zsh"

"$install_prefix/bin/codex" --version
```

`SHA256SUMS` verifies the archive and `BUILD_INFO.json`. The installer creates
`<prefix>/libexec/codex-otel-redacted/<commit>/` and links `<prefix>/bin/codex`
to that package. It preserves older package directories. Both binaries remain
inside the same package, beside its `codex-package.json` metadata.

## Verify the installation

- `codex --version` must print `codex-cli 0.1.0 (Novum; commit <full commit>)`.
  Compare that commit with `sourceCommit` in `BUILD_INFO.json`. `codex -V`
  prints the shorter release version.
- Confirm `command -v codex` selects the prefix you installed. Add its `bin`
  directory to `PATH` if needed, and start a new shell or clear its command cache.
- Start a new agent session with Code Mode enabled and run `text("hello")`
  through its `exec` tool. A successful result verifies CLI/host communication.
- Existing processes keep running their old binaries. Start new sessions to
  use the release. Retain the old package until this check passes.

These are local development builds, not Apple-notarized releases. If macOS
blocks execution, verify the checksum and use macOS's per-app approval flow.
Do not disable Gatekeeper globally.

## Fork version policy

Use independent versions starting with `0.1.0`, and increase the version for
each published release. Tags use `novum-v<version>`. Pass the same version to
the package builder's `--package-version` option; the runtime reads it from
`codex-package.json`. `--version` also reports the compiled source commit.

Keep Cargo's upstream workspace version unchanged. When building both binaries,
set `STABLE_GIT_COMMIT` to the full source commit and build them in one invocation.
Record that commit, the upstream commit, target, build profile, and binary
checksums in `BUILD_INFO.json`. Never reuse a published version for different
binary contents. Install future fork releases with this procedure; the upstream
`codex update` command is not the fork's update path.
