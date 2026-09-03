#!/usr/bin/env bash
# Install a locally built Codex package into a content-addressed prefix.
#
# The package builder is the only supported way to install this fork: it emits
# `codex-package.json` beside `bin/`, which is what lets `InstallContext` locate
# `codex-code-mode-host` inside the package instead of guessing a sibling of the
# invoked executable. Installing a bare `codex` binary leaves host resolution to
# fall back on `<prefix>/bin/codex-code-mode-host`, where a symlink from an older
# install can survive and get picked up long after its `codex` is gone.

set -euo pipefail

PREFIX="${PREFIX:-/opt/homebrew}"
PACKAGE_NAME="${PACKAGE_NAME:-codex-otel-redacted}"

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

case "$(uname -s)/$(uname -m)" in
  Darwin/arm64) TARGET=aarch64-apple-darwin ;;
  Darwin/x86_64) TARGET=x86_64-apple-darwin ;;
  *) echo "This installer only supports macOS; got $(uname -s)/$(uname -m)." >&2; exit 1 ;;
esac

revision="$(git rev-parse --short HEAD)"
if ! git diff --quiet HEAD; then
  revision="${revision}-dirty"
  echo "warning: working tree is dirty; installing as ${revision}" >&2
fi
package_dir="${PREFIX}/libexec/${PACKAGE_NAME}/${revision}"

# Reuse locally cached Codex-built V8 artifacts when present. The `v8` crate's
# own download path 404s for the sandbox variant this fork enables, so the
# builder otherwise fetches them from the openai/codex release tag. It honors
# these overrides only when both are set.
if [[ -z "${RUSTY_V8_ARCHIVE:-}" && -z "${RUSTY_V8_SRC_BINDING_PATH:-}" ]]; then
  v8_version="$(PYTHONPATH="${REPO_ROOT}/scripts" CODEX_REPO_ROOT="$REPO_ROOT" python3 -c \
    'from codex_package.v8 import resolved_v8_crate_version; print(resolved_v8_crate_version())')"
  cache_dir="${HOME}/.cache/codex/rusty-v8-v${v8_version}-${TARGET}"
  archive="${cache_dir}/librusty_v8_ptrcomp_sandbox_release_${TARGET}.a.gz"
  binding="${cache_dir}/src_binding_ptrcomp_sandbox_release_${TARGET}.rs"
  if [[ -f "$archive" && -f "$binding" ]]; then
    echo "Using cached V8 artifacts from ${cache_dir}"
    export RUSTY_V8_ARCHIVE="$archive"
    export RUSTY_V8_SRC_BINDING_PATH="$binding"
  fi
fi

echo "Assembling package at ${package_dir}"
just assemble-codex-package \
  --target "$TARGET" \
  --package-dir "$package_dir" \
  --force \
  "$@"

# `validate_package_dir` already fails the build when the host is missing; check
# again so a hand-edited package can never be linked into the prefix.
host="${package_dir}/bin/codex-code-mode-host"
if [[ ! -x "$host" ]]; then
  echo "Refusing to link a package without an executable ${host}." >&2
  exit 1
fi

install -d -m 0755 "${PREFIX}/bin"
ln -sfn "${package_dir}/bin/codex" "${PREFIX}/bin/codex"

# Older installs linked the host directly into the prefix. It is dead weight now
# that the package layout resolves the host, and a stale one is actively harmful.
rm -f "${PREFIX}/bin/codex-code-mode-host"

echo
echo "Installed:"
ls -l "${PREFIX}/bin/codex"
"${PREFIX}/bin/codex" --version
