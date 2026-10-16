# Baseline — 2026-10-15 (seekr 0.5.0, post-AiWrangler integration)

First full-corpus run against the real stack: qwen3.5 worker (Ollama),
pre-planned DAGs, real AiWrangler gates (jev-1.13.0, strict), 20-minute
budget per fixture.

```
fixture        steps  dag   status        wrk calls  gates p/f  mut  rb  esc  worker tok  gate tok  sec
base32          0/2   ✗    timeout(124)  —          1/0        3    0   0    —           0         1200
csv_fields      0/2   ✗    step failed   16         1/0        2    0   1    39,674      0         150
lru_cache       0/2   ✗    step failed   17         1/0        4    0   1    36,989      0         390
token_bucket    0/2   ✗    step failed   19         1/1        3    3   1    51,768      19,911    193
wal_writer      0/2   ✗    step failed   16         1/0        2    0   1    45,612      0         181
─────────────────────────────────────────────────────────────────────────────────────────────────────
step completion 0/10 (0%)   [target ≥ 80%]
DAG completion  0/5  (0%)   [target ≥ 60%]
frontier calls  4    worker tokens 174,043 · gate tokens 19,911
```

## What the number hides (all positive deltas vs. the morning's live run)

- **The worker writes now.** Every fixture produced 2–4 real mutations;
  the morning run produced reads-only loops. The anti-stall nudges and
  stall escalation changed the failure mode from "never acts" to "acts
  but doesn't converge."
- **The full gate chain fired live.** Plan gates passed on all five;
  token_bucket's step passed deterministic `cargo test` verification and
  was then rejected by the AiWrangler check battery with cited findings
  (19.9k judgment tokens) → rollback → retry. Integration verified
  end-to-end against the real server.
- **Stall escalation works**: 4/5 fixtures reached the frontier tier.

## The actual bottleneck (Phase 3 target)

Every fixture died on step 1 at deterministic verification: the worker's
first implementations don't compile-and-pass within 3 attempts + one
guidance round. The loop mechanics (rollback, findings feedback,
escalation) all work; convergence doesn't.

## Iteration history

| Run | Steps | Mutations/run | Notes |
|---|---|---|---|
| baseline (0.5.0) | 0/10 | 2–4 | reads-mostly stalls; gate chain verified live |
| iter 1 (seed + inline diagnostics + distilled failures) | 0/10 | 3–7 | writes up 2×, reads down; **governor bug found: stalls discarded unverified work** |
| iter 2 (auto-verify on stall; frontier empty-reply retry) | 0/10 | 2–8 | verify gates engage on stalls; token_bucket reaches the check gate with cited FIX findings; wal_writer incomplete (bench aborted, model server cold) |
| iter 3 (keep-diff attempts; 2-attempt policy; silent-fallback rule; test-success nudge) | 0/10 | 2–3 | loop is lean and complete (7–10 calls, ~95k worker tokens total, every fixture reaches escalation) — but qwen's module-scale code is self-inconsistent and doesn't converge in 2 attempts |
| mini-bench (iter 3, single-function tasks) | **2/2** | 1–2 | word_count ~80s, slugify 182–318s — **worker-only, zero frontier calls, gate-approved** |

## Where the line is

Single-function tasks: the local worker carries them alone, end to end.
Whole-module tasks (the five corpus fixtures): the worker's code
disagrees with itself across a file (tuple designs matched against
non-tuple maps, helpers referenced but never defined) and 2 attempts +
one guidance round don't close it. The loop is no longer the limiter —
the worker tier is, at this task size.

Options, per the product thesis (cheap labor, expensive supervision):

1. **Per-step model routing** — the planner tags step difficulty; hard
   steps go to a mid-tier API model (still ~10× cheaper than
   frontier-only), easy ones stay local. Needs planner support.
2. **Finer task split** — planner decomposes modules into
   function-sized steps (mini-bench scale, where locals win).
   More frontier planning tokens, same free grinding.
3. **Escalate harder** — frontier patch on FIRST failed verification
   instead of after guidance. Cheapest to build (the mechanism exists);
   spends more frontier tokens per run.

