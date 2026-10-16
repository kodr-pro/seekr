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

Ranked levers, to be judged against this baseline:

1. **Seed the target file into attempt-start context** — remove the
   read/search phase entirely; the worker starts from the code in hand.
2. **Compile feedback inside the attempt loop** — auto-run
   `cargo check` after each mutation (the `diagnostics` tool exists for
   exactly this) so syntax errors burn one action, not one attempt.
3. **Curate verify-failure context** — point the worker at the exact
   failing test and diff-worthy lines instead of the full output blob.
4. **Escalate at mid-attempt stall** — zero mutations by attempt
   midpoint goes straight to guidance.
