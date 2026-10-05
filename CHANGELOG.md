# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.4.0] - 2026-10-05

Jev Jockey: seekr becomes an autonomous, cost-guarded agent engine.

### Added
- **Jev (TypeSafe AI) client**: noul/choice/score judgments over `/v1/systemone`,
  sha256 content-addressed disk cache, retry with backoff + Retry-After,
  fail-closed egress/no-key semantics.
- **Task DAG planner**: frontier decomposes goals into a validated DAG
  (topological order, per-step invariants, allowed paths, deterministic
  verification commands) with bounded clarification rounds.
- **Bounded-context worker**: fresh two-message context per attempt,
  restricted five-tool set, single-tool-call discipline, one repair round.
- **Interceptor**: deterministic-first gating (fail-closed path sandbox,
  shell blocklist, duplicate fingerprints, read-only command allowlist)
  then one batched Jev call — scope ≥ 0.85, steering < 0.5, novelty > 1.0.
- **Git worktree sandbox**: isolated `jj/<run-id>` worktrees, checkpoint
  commits on verified steps, rollback including untracked files,
  atomic frontier patches via `git apply --check`.
- **Triage & escalation**: Jev choice (syntax_fix / read_context /
  deadlock); frontier escalation with minimal payloads, guidance or
  verified diff, hard call budgets.
- **Cost ledger & runs**: per-tier token accounting, resumable run state
  (`seekr runs` / `seekr resume`), `seekr merge` / `seekr clean`.
- **New TUI**: goal intake → clarify → plan review → grind dashboard
  (DAG panel, governor telemetry, activity log, ledger).
- Token usage fixes: `stream_options.include_usage` on OpenAI streaming,
  usage captured on non-streaming calls, `reasoning_effort` passthrough.

### Removed
- Interactive chat loop, daemon, subagents, MCP/LSP integrations, script
  skills, session persistence (superseded; preserved on `main`).

## [0.2.1] - 2026-03-22

Security and Performance Overhaul Update.

### Added
- **Secure Keyring Storage:** API keys are now securely managed via the OS keyring rather than plain-text configuration files.
- **Provider Abstraction:** Introduced a new generic `Provider` trait for scaling multiple LLM APIs.
- **Anthropic Streaming & Tool-Use support:** Implemented native SSE parsing and standardized tool payload routing for Anthropic API.
- **File Sandboxing:** Added strict directory traversal protections limiting file editing strictly back to the defined workspace directory.
- **SSRF Network Protection:** Enabled security checks for external network requests, blocking interaction with private subnets natively.
- **Environment API keys:** Added support for `SEEKR_API_KEY` global override alongside provider-specific bindings.
- **Semantic Version Checking:** Built-in update notifier now gracefully verifies updates over Git using the `semver` standard instead of raw string matching.

### Changed
- Replaced manual `Seekr/0.1` User-Agent strings with dynamically loaded Cargo version.
- Refactored the core monolithic `App` container into loosely coupled decoupled structures (`UiState`, `AgentState`, `SessionState`).
- Standardized inline API payload construction without redundant `clone()` allocations.
- Updated Shell component to rely exclusively on user-configurable blocklists rather than static rules.

### Fixed
- Fixed critical UTF-8 slicing panic when truncating multi-byte chat session titles.
- Fixed an out-of-bounds pointer crash resulting from modifying the provider index config.
- Corrected API validation behavior failing selectively across Anthropic and OpenAI model boundaries.

## [0.2.0] - 2026-03-12

### Added
- Initial stable release with core Anthropic/OpenAI integration and Multi-Agent processing.
