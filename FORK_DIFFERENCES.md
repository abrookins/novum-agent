# Fork differences from upstream

This file is the maintained inventory of behavior that must persist when an
upstream release is merged into this fork. It is not a generated diff.

## Current comparison

- Upstream reference: `rust-v0.156.1` at `b412ff32c417f855c2b2d1581b77058eed87c84b`
- Fork reference before merge: `main` at `6be24a90d448b9bab7f0c1e0847d0145bb1258fa`
- Merge branch: `sync/upstream-0.156.1`
- Merge base: `a526f54b005f0647dec041f26a67356be88b15fe`
- Post-merge fork delta from the tag: 87 changed files, 4,135 insertions, and
  1,072 deletions.

## Required fork behavior

### Release identity and installation

- Preserve the fork installation link in `README.md` and the agent procedure in
  `FORK_INSTRUCTIONS.md`.
- `codex -V` reports the package version; `codex --version` also reports Novum
  and the compiled commit. Use independent `novum-v<version>` release tags.
- Package the CLI and Code Mode host together. Preserve package metadata and
  bundled resources when installing prebuilt release assets.
- Main ownership paths: `codex-rs/cli/src/main.rs`,
  `codex-rs/cli/tests/version.rs`, and `scripts/install-local-codex.sh`.
- Daemon lifecycle tests must isolate the installer's home and launcher paths
  and prevent downloads of upstream executables.

### Telemetry privacy

- Exported traces, metrics, and telemetry logs must not contain sensitive
  prompts, tool arguments or output, identifiers, hostnames, URLs, headers,
  request IDs, IP addresses, or error text.
- Preserve only approved aggregate and diagnostic fields. The sanitizer must
  run before telemetry export.
- Keep the log-only events for OpenAI file uploads free of file IDs, hosts, and
  provider request identifiers.
- Main ownership paths: `codex-rs/otel/src/trace_sanitizer.rs`,
  `codex-rs/otel/src/{provider.rs,metrics/,events/}`, and the telemetry call
  sites in `codex-rs/core/`, `codex-rs/codex-api/`, and
  `codex-rs/network-proxy/`.
- Tests: `just test -p codex-otel` and the focused `codex-core` telemetry
  tests. Use canary values to prove sensitive fields are absent.

### Bounded and recoverable tool output

- Keep model-visible tool output bounded. Oversized output must be captured in
  a session-owned temporary store and represented to the model by a recoverable
  handle, not discarded.
- Preserve the `tool_output` tool, including bounded reads and literal search
  with context. Handles must remain opaque, scoped to the session, and expire
  with it.
- Preserve live unified-exec stream capture so completed and running commands
  can expose recoverable output without expanding the model context.
- Keep the shared output representation in `codex-rs/tools/src/tool_output.rs`
  aligned with Code Mode and unified exec.
- Main ownership paths: `codex-rs/core/src/tools/{context.rs,output_store.rs,
  registry.rs,handlers/tool_output.rs,runtimes/unified_exec.rs}`, and
  `codex-rs/core/src/unified_exec/`.
- Tests: focused `codex-core` tool-output, unified-exec, and Code Mode tests.

### Local reliability and validation policy

- Keep the Nextest serialization rules for OTEL HTTP loopback and full-turn
  tests; they prevent global-provider and local-server contention.
- Keep the private macOS fork validation policy in `AGENTS.md`: use affected
  tests, scoped linting, formatting, a binary build, and a focused smoke test;
  do not default to the full workspace matrix.

## Upstream merge checklist

1. Read this file before resolving conflicts.
2. Compare the target upstream release with the merge result and confirm each
   required behavior above still exists.
3. Run the affected tests, including the telemetry privacy and tool-output
   tests when those paths change.
4. Update the comparison references, scope, ownership paths, and validation
   commands in this file as the fork changes.
