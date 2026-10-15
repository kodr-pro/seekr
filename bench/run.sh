#!/usr/bin/env bash
# Benchmark harness: runs every fixture in bench/fixtures through a real
# autonomous seekr run and reports step completion, gate verdicts, tokens,
# and wall time. Fixtures are copied to temp dirs so runs never touch the
# corpus.
#
# Usage:
#   bench/run.sh                     # all fixtures
#   bench/run.sh lru_cache wal_writer  # subset
#
# Env:
#   SEEKR_BIN   path to the seekr binary (default target/release/seekr)
#   SEEKR_BENCH_OUT  results file (default bench/results.jsonl)

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SEEKR_BIN="${SEEKR_BIN:-$ROOT/target/release/seekr}"
OUT="${SEEKR_BENCH_OUT:-$ROOT/bench/results.jsonl}"
FIXTURES=("$ROOT/bench/fixtures")

if [ "$#" -gt 0 ]; then
  SELECTED=("$@")
else
  SELECTED=()
  for d in "$ROOT"/bench/fixtures/*/; do
    SELECTED+=("$(basename "$d")")
  done
fi

echo "seekr bench — $(date -Is)"
echo "binary: $SEEKR_BIN"
"$SEEKR_BIN" --version || true
echo

mkdir -p "$(dirname "$OUT")"
: > "$OUT.tmp"

for name in "${SELECTED[@]}"; do
  src="$ROOT/bench/fixtures/$name"
  [ -d "$src" ] || { echo "no such fixture: $name" >&2; exit 1; }
  work="$(mktemp -d "/tmp/opencode/bench-$name-XXXXXX")"
  cp -r "$src/." "$work/"
  state="$work/.state"
  log="$work/run.jsonl"

  echo "── $name ──────────────────────────────"
  start=$(date +%s)
  set +e
  (cd "$work" && SEEKR_STATE_DIR="$state" timeout 3600 \
     "$SEEKR_BIN" run . --plan plan.json --auto --headless --json) \
     > "$log" 2> "$work/stderr.log"
  rc=$?
  set -e
  end=$(date +%s)

  python3 "$ROOT/bench/collect.py" \
    --fixture "$name" --log "$log" --worktree "$work" \
    --secs $((end - start)) --exit "$rc" --results "$OUT.tmp"
  if [ $rc -ne 0 ]; then
    echo "  (seekr exited $rc; stderr tail:)"
    tail -3 "$work/stderr.log" | sed 's/^/    /'
  fi
done

echo
python3 "$ROOT/bench/summarize.py" --results "$OUT.tmp"
mv "$OUT.tmp" "$OUT"
echo
echo "results appended to $OUT"
