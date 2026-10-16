# Seekr

**OpenCode when you're driving. Seekr when you're sleeping.**

Seekr is an autonomous coding agent for long, well-specified tasks: you
queue the work, walk away, and come back to finished branches with a
reviewable paper trail — at a fraction of the cost of running a frontier
model end to end.

```
queue a task → the frontier plans it → a free local model grinds it
→ AiWrangler gates every step against your ground truth → you review
the findings and merge
```

## Why this shape

Three facts about models, and what they add up to:

| Tier | Cost | Quality |
|---|---|---|
| Frontier API models | expensive per token | accurate, plans well |
| Local models (Ollama, vLLM) | free, runs all night | drift on long tasks |
| [AiWrangler](https://github.com/kodr-pro/AiWrangler) judgments | cheap | calibrated, citable |

Big-model agents cost dollars per task. Local-model agents drift within
the hour. Seekr's bet: **cheap labor, expensive supervision, real
gates** — the frontier only writes the plan and occasionally finishes a
stuck step; the local model does the overwhelming majority of the work;
AiWrangler's battery (scope discipline, claims-vs-diff, silent defaults,
injection defense, test quality) judges every completed step against the
documents that actually govern your repo: your spec, your review rules,
your divergence rulings.

The only real cost is server horsepower. Completed runs report their
ledger: when the local tier carries the task, frontier calls read zero.

## What it's for (and not for)

**Shines** — verification-friendly, high-volume work:

- test backfill and characterization tests
- dependency and API migrations
- lint debt, doc drift, mechanical refactors
- any task where "done" can be expressed as a command that exits 0

**Wrong tool** — novel feature work with no test suite, exploratory
spikes, anything where you can't state the invariants upfront. Without a
deterministic verification command, the gates have nothing to anchor to
and drift comes back. Use OpenCode interactively for those.

## How a run works

1. **Plan** — the frontier decomposes your goal into a rigid DAG: per-step
   invariants, allowed paths, and a deterministic verification command.
   AiWrangler's plan gate judges it against your ground truth first:
   ambiguous goals stop for answers (CLARIFY), one-way doors stop for
   the owner (HUMAN). Nothing touches disk until the plan passes.
2. **Grind** — the local worker executes one small action at a time
   inside a fail-closed interceptor: path sandbox, shell blocklist,
   duplicate-fingerprint loop breaker. Attempts keep their diff — a
   near-miss iterates instead of rebuilding — and compile feedback runs
   inline so a syntax error costs one action, not one attempt.
3. **Gate** — every completed step passes its verification command, then
   AiWrangler's check battery over the dirty worktree. PASS checkpoints;
   FIX REQUIRED feeds the cited findings to the next attempt; HUMAN
   REVIEW parks the step until `seekr ack`; DEGRADED fails closed.
4. **Finish** — two stalled attempts and the frontier steps in: guidance
   first, a verified patch if guidance fails. Either way the step
   completes or fails with its reasons recorded.
5. **Review** — `seekr summary <run-id>` shows you the three things
   that need attention: parks, failures, review-severity findings —
   findings that fold into the merge commit as review notes. Your main
   checkout is never touched; every run lives on its own branch in its
   own worktree.

## Install

```sh
cargo install --path .
```

Requires: git, Node 20+ with
[AiWrangler](https://github.com/kodr-pro/AiWrangler) built, a local
OpenAI-compatible endpoint (Ollama, vLLM, llama.cpp), a frontier
provider key, and `TYPESAFE_API_KEY` for semantic judgments.

## Configure

`~/.config/seekr/config.toml`:

```toml
[[providers]]
name = "qwen-local"
base_url = "http://server:11434/v1"
model = "qwen3.5:latest"

[[providers]]
name = "glm-frontier"
key = "..."                       # or GLM_FRONTIER_API_KEY env
base_url = "https://api.z.ai/api/coding/paas/v4"
model = "glm-5.3"

[jj]
worker_provider = "qwen-local"
frontier_provider = "glm-frontier"
max_attempts_per_step = 2         # local attempts before the frontier
wrangler_command = "node /path/to/AiWrangler/dist/mcp.js"
```

## Use

```sh
seekr .                            # TUI: goal → clarify → plan review → dashboard
seekr run . --goal "backfill tests for src/parser/*"
seekr summary <run-id>             # the return-after-hours view
seekr ack <run-id>                 # review / acknowledge HUMAN findings
seekr merge <run-id>               # merge back; findings become review notes
seekr runs · logs · resume · clean · rm · doctor · completions
```

## Safety model

- **Fail-closed gates** — unreachable semantic layer means steps can't
  pass, not that they wave through.
- **Deterministic interception** — path escapes, blocklisted commands,
  and exact-repeat failures are rejected before execution.
- **Reversibility** — checkpoints per verified step; nothing unverified
  ever lands on your branch; your checkout is never touched.
- **Audit** — AiWrangler's append-only ledger records every verdict,
  finding, and acknowledgment, bound to the exact diff.

## Development

```sh
cargo test                                   # unit + wiremock + mock-gate e2e
bench/mini.sh                                # 2-fixture smoke bench (minutes)
bench/run.sh                                 # full graded corpus
SEEKR_JJ_TEST_URL=http://server:11434/v1 cargo test -- --ignored   # live
```

`mock_aiwrangler` (`src/bin/`) is a scenario-driven stand-in for the
gate server; the whole loop is testable offline.

Layout: `src/wrangler` (AiWrangler MCP client), `src/jockey` (dag,
planner, worker, interceptor, driver, ledger, cli), `src/sandbox` (git
worktree, path containment, command exec), `src/ui` (TUI), `src/api`
(providers).
