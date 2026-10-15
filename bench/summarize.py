#!/usr/bin/env python3
"""Summarize benchmark result records into the release-bar report."""

import argparse
import json


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--results", required=True)
    args = ap.parse_args()

    rows = []
    with open(args.results, encoding="utf-8") as f:
        for line in f:
            line = line.strip()
            if line:
                rows.append(json.loads(line))

    if not rows:
        print("no results")
        return

    total_steps = sum(r["steps_planned"] for r in rows)
    done_steps = sum(r["steps_completed"] for r in rows)
    dags_done = sum(1 for r in rows if r["dag_completed"])
    worker_tok = sum(r["worker_tokens"] for r in rows)
    gate_tok = sum(r["gate_tokens"] for r in rows)
    frontier = sum(r["frontier_calls"] for r in rows)
    wall = sum(r["wall_secs"] for r in rows)

    hdr = f"{'fixture':<14} {'steps':>7} {'dag':>4} {'status':<24} {'wrk':>5} {'gates p/f/h':>12} {'mut':>4} {'rb':>3} {'esc':>4} {'worker tok':>11} {'gate tok':>9} {'sec':>5}"
    print(hdr)
    print("-" * len(hdr))
    for r in sorted(rows, key=lambda x: x["fixture"]):
        g = r["gates"]
        print(
            f"{r['fixture']:<14}"
            f" {r['steps_completed']}/{r['steps_planned']:<5}"
            f" {'✓' if r['dag_completed'] else '✗':>4}"
            f" {r['status'][:24]:<24}"
            f" {r['worker_calls']:>5}"
            f" {g['pass']}/{g['fix_required']}/{g['human']:>7}"
            f" {r['mutations']:>4}"
            f" {r['rollbacks']:>3}"
            f" {r['escalations']:>4}"
            f" {r['worker_tokens']:>11}"
            f" {r['gate_tokens']:>9}"
            f" {r['wall_secs']:>5}"
        )

    print("-" * len(hdr))
    print(
        f"step completion : {done_steps}/{total_steps}"
        f" ({100.0 * done_steps / max(total_steps, 1):.0f}%)   [target ≥ 80%]"
    )
    print(
        f"DAG completion  : {dags_done}/{len(rows)}"
        f" ({100.0 * dags_done / len(rows):.0f}%)   [target ≥ 60%]"
    )
    print(
        f"frontier calls  : {frontier} (0 completions after planning is the goal)"
    )
    print(f"tokens          : worker {worker_tok:,} · gates {gate_tok:,}")
    print(f"wall clock      : {wall}s total")


if __name__ == "__main__":
    main()
