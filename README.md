# Seekr

An autonomous, cost-guarded coding agent. You describe a goal; a frontier
model turns it into a rigid task DAG; a cheap local model grinds through
every step; [AiWrangler](https://github.com/kodr-pro/AiWrangler) —
ground-truth guardrails for coding agents — gates the plan and every
completed step; deterministic verification runs between attempts; git
worktrees make every run reversible.

```
              [goal]
                │
        Phase 0: Plan (frontier, 1-2 calls)
        clarify-or-DAG, schema + topo validated
        then AiWrangler plan gate: PASS / CLARIFY / HUMAN
        (context pack becomes the worker's ground truth)
                │
   ┌────────► [DAG step] ≤3 local attempts
   │            │
   │   Phase 1: Worker (local SLM, fresh bounded context)
   │   proposes ONE of: read_file write_file edit_file
   │                     run_command finish_step
   │            │
   │   Phase 2: Interceptor (deterministic, fail-closed)
   │   ├─ path sandbox: .., absolute, symlink escapes rejected
   │   ├─ shell blocklist, read-only command allowlist
   │   └─ duplicate-fingerprint loop breaker
   │            │
   │   Phase 3: Execution + verification + AiWrangler check gate
   │   finish_step → verification command → wrangler_check over the
   │   dirty worktree (scope discipline, claims-vs-diff, injection
   │   defense, silent defaults, test quality)
   │     PASS → checkpoint commit
   │     FIX REQUIRED → rollback + cited findings as next context
   │     HUMAN REVIEW → step parks until `seekr ack <run-id>`
   │     DEGRADED → fail-closed unless --allow-degraded
   │            ▼
   │   Phase 4: Triage (deterministic) + escalation
   │   syntax_fix → retry with stderr
   │   read_context → inject the file the error points at
   │   deadlock / attempts exhausted → escalate to frontier
   │   (minimal payload; ```diff patch verified via git apply, or
   │    GUIDANCE injected once) ── hard cap on frontier calls
   └──── next ready step
```

## Install

```sh
cargo install --path .
```

Requires: git, Node 20+ with
[AiWrangler](https://github.com/kodr-pro/AiWrangler) installed
(`npm install && npm run build && npm link`), and `TYPESAFE_API_KEY` for
the semantic gate (without it completed steps come back DEGRADED —
**fail-closed** by design).

## Configure

`~/.config/seekr/config.toml`:

```toml
[[providers]]
name = "qwen-local"
base_url = "http://server:11434/v1"     # any OpenAI-compatible endpoint
model = "qwen3.5:latest"

[[providers]]
name = "glm-frontier"
key = "..."                             # or <NAME>_API_KEY env
base_url = "https://api.z.ai/api/coding/paas/v4"
model = "glm-5.3"

[jj]
worker_provider = "qwen-local"
frontier_provider = "glm-frontier"
max_attempts_per_step = 3
max_frontier_calls = 5
step_timeout_secs = 3600
# How seekr launches the AiWrangler MCP server (argv, whitespace-split).
# Default: "aiwrangler-mcp" from PATH.
wrangler_command = "node /path/to/AiWrangler/dist/mcp.js"
```

Env: `TYPESAFE_API_KEY` (semantic judgments), `AIWRANGLER_EGRESS=off`
(deterministic-only, zero-egress gates), `SEEKR_WORKER_REASONING=none`
(recommended for Ollama thinking models — 2s/turn instead of 70s).

### Ground truth

AiWrangler judges against the documents that govern your project. If the
target repo commits an `aiwrangler.config.json` (pointing at a protocol
spec, review rules, divergence rulings, open questions), it is present in
every run worktree automatically and its ground truth governs. Otherwise
seekr generates a minimal gate config per run (default gates, ledger in
the run state dir) — scope/injection/test-quality checks still run, just
without project-specific corpus.

## Use

```sh
seekr .                   # TUI in this repo: goal → clarify → plan review → grind dashboard
seekr /path/to/repo       # same, elsewhere
seekr run . --goal "..."  # autonomous run (TUI in a terminal, headless when piped)
seekr run . --goal - < goal.txt          # goal from stdin
seekr run . --plan dag.json --auto       # pre-planned DAG, no clarifications
seekr run . --goal "..." --headless --json | jq -c 'select(.type=="step_completed")'
seekr plan --goal "..."   # print a TaskDag JSON without running it
seekr runs                # list runs (state + status)
seekr logs <run-id>       # inspect a run's event log (--lines N)
seekr resume <run-id>     # resume an interrupted run
seekr ack <run-id>        # list AiWrangler human-review findings for a run
seekr ack <run-id> --rules RULE-10 --note "approved: ..."   # acknowledge them
seekr merge <run-id>      # merge the run's branch back, clean its worktree
seekr clean <run-id>      # drop the worktree, keep the branch
seekr rm <run-id>         # delete run state + worktree (branch kept)
seekr doctor              # config/roles/aiwrangler/git diagnostics
seekr completions zsh     # shell completions (bash|zsh|fish|...)
```

In the TUI: **Ctrl+G** opens the control center (runs browser with
attach/resume/merge/clean, provider CRUD with live key tests and worker/
frontier role assignment, `[jj]` settings editor, key reference). **?**
shows help anywhere, **Ctrl+C twice** quits (a run interrupted this way
can be resumed with `seekr resume <id>`). The grind dashboard shows a
tree-style DAG with a progress gauge, live gate verdicts, per-tier token
share gauges, and a scrollable activity log.

Output lands on branch `jj/<run-id>` in a linked worktree under
`<repo>/../.jj-worktrees/` — your main checkout is never touched.

## Safety model

- **Fail-closed gates**: AiWrangler unreachable / no key / egress off →
  completed steps cannot pass verification; runs sink rather than wave
  bad work through. `--allow-degraded` opts out explicitly.
- **Deterministic interceptor**: `..`, absolute escapes, symlink escapes,
  sibling-prefix confusion all rejected; empty `allowed_paths` = read-only
  step; shell blocklist; exact-duplicate failed actions are rejected as
  loops.
- **Ground-truth gating**: the plan gate refuses ambiguous goals
  (CLARIFY) and one-way doors (HUMAN) before any code is written; the
  check gate reads the whole dirty diff and the worker's own summary and
  catches claims that outrun the diff, scope creep, silent defaults,
  injected steering text, and missing tests.
- **Rollback**: every failed attempt is reverted to the last checkpoint,
  including untracked files the worker created.
- **Budgets**: attempts per step, frontier calls per run, per-tier token
  ledger, wall-clock timeouts per step and per verification command.
- **Audit trail**: AiWrangler's append-only ledger records every gate
  verdict, finding, and owner acknowledgment (`<run state>/wrangler/`
  for generated configs, or `.aiwrangler/` in projects that commit one).

## Cost model

A run's judgment tokens are the AiWrangler semantic battery, batched once
per completed step (plus the plan gate) instead of once per action;
identical questions hit AiWrangler's content-addressed disk cache. The
ledger tracks worker / gate / frontier tokens separately in the TUI and
`seekr logs`.

## Development

```sh
cargo test                                # unit + wiremock + mock-gate e2e suites
cargo test --test jockey_e2e_tests        # governor loop vs mocks + mock-aiwrangler
SEEKR_JJ_TEST_URL=http://server:11434/v1 SEEKR_JJ_TEST_MODEL=qwen3.5:latest \
  cargo test -- --ignored                 # live planner/worker tests
```

`mock_aiwrangler` (`src/bin/`) is a hermetic stand-in for the MCP server:
it serves canned gate verdicts from a scenario file, so the whole
governor loop is testable offline. Run it with no arguments for a healthy
PASS scenario.

Layout: `src/wrangler` (AiWrangler MCP client), `src/jockey` (dag,
planner, worker, interceptor, driver, ledger, cli), `src/sandbox` (git
worktree, path containment, command exec), `src/ui/jockey.rs` (TUI),
`src/api` (OpenAI/Anthropic/Gemini providers).
