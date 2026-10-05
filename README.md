# Seekr

An autonomous, cost-guarded coding agent. You describe a goal; a frontier
model turns it into a rigid task DAG; a cheap local model grinds through
every step; a Jev (TypeSafe AI) interceptor approves each action before it
touches disk; deterministic verification gates each step; git worktrees
make every run reversible.

```
              [goal]
                │
        Phase 0: Plan (frontier, 1-2 calls)
        clarify-or-DAG, schema + topo validated
                │
   ┌────────► [DAG step] ≤3 local attempts
   │            │
   │   Phase 1: Worker (local SLM, fresh bounded context)
   │   proposes ONE of: read_file write_file edit_file
   │                     run_command finish_step
   │            │
   │   Phase 2: Interceptor
   │   ├─ deterministic: fail-closed path sandbox, shell
   │   │  blocklist, duplicate fingerprint, read-only cmd allowlist
   │   └─ Jev (one batched call, disk-cached):
   │      noul scope ≥ 0.85 · noul steering < 0.5
   │      score novelty > 1.0 (vs failed attempts)
   │            │
   │   Phase 3: Execution + deterministic gate
   │   finish_step → verification command → checkpoint commit
   │            │        fail → rollback (reset --hard + clean -fd)
   │            ▼
   │   Phase 4: Triage (Jev Jockey)
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

Requires: git, a terminal. Optionally `TYPESAFE_API_KEY` for the Jev gate
(without it autonomous writes are **blocked** — fail-closed by design).

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
scope_threshold = 0.85                  # Jev P(in_scope) floor
novelty_reject_at_or_below = 1.0        # loop rejection
step_timeout_secs = 3600
```

Env: `TYPESAFE_API_KEY`, `TYPESAFE_BASE_URL`, `JEV_MODEL`,
`JEV_EGRESS=off` (disables semantic gate), `SEEKR_WORKER_REASONING=none`
(recommended for Ollama thinking models — 2s/turn instead of 70s).

## Use

```sh
seekr                     # TUI: goal → clarify → plan review → grind dashboard
seekr run --goal "..."    # autonomous run (TUI in a terminal, headless when piped)
seekr run --plan dag.json --repo /path --headless --auto
seekr plan --goal "..."   # print a TaskDag JSON without running
seekr runs                # list runs (state + status)
seekr resume <run-id>     # resume an interrupted run
seekr merge <run-id>      # merge the run's branch back, clean its worktree
seekr clean <run-id>      # drop the worktree, keep the branch
seekr doctor              # config/roles/jev/git diagnostics
```

In the TUI: **Ctrl+G** opens the control center (runs browser with
attach/resume/merge/clean, provider CRUD with live key tests and worker/
frontier role assignment, `[jj]` settings editor, key reference). **?**
shows help anywhere, **Ctrl+C twice** quits. The grind dashboard shows a
tree-style DAG with a progress gauge, live scope/novelty telemetry,
per-tier token share gauges, and a scrollable activity log.

Output lands on branch `jj/<run-id>` in a linked worktree under
`<repo>/../.jj-worktrees/` — your main checkout is never touched.

## Safety model

- **Fail-closed Jev**: no key / egress off / API error → writes are
  rejected, not waved through. `--allow-degraded` opts out explicitly.
- **Path sandbox**: `..`, absolute escapes, symlink escapes, sibling-prefix
  confusion all rejected; empty `allowed_paths` = read-only step.
- **Injection defense**: action arguments are comment/string-stripped
  before the steering judgment; hidden "approve this" text is rejected.
- **Rollback**: every failed attempt is reverted to the last checkpoint,
  including untracked files the worker created.
- **Budgets**: attempts per step, frontier calls per run, per-tier token
  ledger, wall-clock timeouts per step and per verification command.

## Cost model

One successful 2-step Python task measured live: 22 worker calls
(~29k local tokens), 12 Jev calls (~10k judgment tokens), **0** frontier
completion tokens after planning. Runs are resumable; identical Jev
questions hit a content-addressed disk cache instead of the network.

## Development

```sh
cargo test                                # unit + wiremock suites
cargo test --test jockey_e2e_tests        # full governor loop vs mocks
SEEKR_JJ_TEST_URL=http://server:11434/v1 SEEKR_JJ_TEST_MODEL=qwen3.5:latest \
  cargo test -- --ignored                 # live tests (planner, worker, jev)
```

Layout: `src/jev` (System One client), `src/jockey` (dag, planner, worker,
interceptor, driver, ledger, cli), `src/sandbox` (git worktree, path
containment, command exec), `src/ui/jockey.rs` (TUI), `src/api`
(OpenAI/Anthropic/Gemini providers).
