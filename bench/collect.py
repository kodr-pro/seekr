#!/usr/bin/env python3
"""Parse one benchmark run's JSONL event log into a result record."""

import argparse
import json
import sys


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--fixture", required=True)
    ap.add_argument("--log", required=True)
    ap.add_argument("--worktree", required=True)
    ap.add_argument("--secs", type=int, required=True)
    ap.add_argument("--exit", type=int, required=True)
    ap.add_argument("--results", required=True)
    args = ap.parse_args()

    steps_planned = steps_completed = steps_failed = 0
    gate_pass = gate_fix = gate_human = gate_degraded = 0
    rollbacks = escalations = frontier_guidance = 0
    mutations = rejections = 0
    ledger = None
    run_status = None
    run_id = None

    with open(args.log, "r", encoding="utf-8", errors="replace") as f:
        for line in f:
            line = line.strip()
            if not line:
                continue
            try:
                ev = json.loads(line)
            except json.JSONDecodeError:
                continue
            t = ev.get("type")
            if t == "step_completed":
                steps_completed += 1
            elif t == "step_failed":
                steps_failed += 1
            elif t == "wrangler_gate":
                action = ev.get("action", "")
                if "FixRequired" in action:
                    gate_fix += 1
                elif "HumanReview" in action:
                    gate_human += 1
                elif "Degraded" in action or "Unreachable" in action or "Inactive" in action:
                    gate_degraded += 1
                elif "Pass" in action:
                    gate_pass += 1
            elif t == "rolled_back":
                rollbacks += 1
            elif t == "escalated":
                escalations += 1
            elif t == "frontier_guidance":
                frontier_guidance += 1
            elif t == "action_rejected":
                rejections += 1
            elif t == "action_executed":
                tool = ev.get("tool", "")
                if tool in ("write_file", "edit_file"):
                    mutations += 1
            elif t == "summary":
                ledger = ev.get("ledger")
                run_status = ev.get("status")
                run_id = ev.get("run_id")

    # steps planned from the plan file
    with open(f"{args.worktree}/plan.json", encoding="utf-8") as f:
        plan = json.load(f)
    steps_planned = len(plan.get("steps", []))

    record = {
        "fixture": args.fixture,
        "ts": __import__("time").strftime("%Y-%m-%dT%H:%M:%S"),
        "run_id": run_id,
        "status": run_status or f"crashed(exit {args.exit})",
        "steps_planned": steps_planned,
        "steps_completed": steps_completed,
        "steps_failed": steps_failed,
        "step_completion_pct": round(
            100.0 * steps_completed / steps_planned, 1
        )
        if steps_planned
        else 0.0,
        "dag_completed": steps_completed == steps_planned and steps_planned > 0,
        "gates": {
            "pass": gate_pass,
            "fix_required": gate_fix,
            "human": gate_human,
            "degraded_or_unreachable": gate_degraded,
        },
        "mutations": mutations,
        "rejections": rejections,
        "rollbacks": rollbacks,
        "escalations": escalations,
        "frontier_guidance": frontier_guidance,
        "worker_tokens": (ledger or {}).get("worker_prompt_tokens", 0)
        + (ledger or {}).get("worker_completion_tokens", 0),
        "gate_tokens": (ledger or {}).get("wrangler_input_tokens", 0)
        + (ledger or {}).get("wrangler_output_tokens", 0),
        "frontier_calls": (ledger or {}).get("frontier_calls", 0),
        "worker_calls": (ledger or {}).get("worker_calls", 0),
        "wall_secs": args.secs,
        "exit": args.exit,
    }
    with open(args.results, "a", encoding="utf-8") as f:
        f.write(json.dumps(record) + "\n")

    ok = "✓" if record["dag_completed"] else "✗"
    print(
        f"  {ok} {record['steps_completed']}/{record['steps_planned']} steps"
        f" · {record['status'][:44]}"
        f" · {record['worker_calls']} worker calls"
        f" · gates pass/fix {gate_pass}/{gate_fix}"
        f" · {record['wall_secs']}s"
    )


if __name__ == "__main__":
    sys.exit(main())
