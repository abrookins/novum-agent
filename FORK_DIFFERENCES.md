# Fork differences from upstream

This file is the maintained inventory of behavior that must persist when
`upstream/main` is merged into this fork. It is not a generated diff.

## Current comparison

- Upstream reference: `upstream/main` at `a25e986323931ec54909b0cd936b612f30c8ce46`
- Fork reference: `main` at `7ffd9bb93`
- Merge base: `a25e986323931ec54909b0cd936b612f30c8ce46`
- Scope: 70 changed files, 3,518 insertions, and 873 deletions.

## Required fork behavior

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
2. Compare `upstream/main...main` after the merge and confirm each required
   behavior above still exists.
3. Run the affected tests, including the telemetry privacy and tool-output
   tests when those paths change.
4. Update the comparison references, scope, ownership paths, and validation
   commands in this file as the fork changes.
