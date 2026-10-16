#!/usr/bin/env bash
# Mini-bench: two single-step fixtures, 3-minute hard budget each.
# Fast feedback loop for invocation/policy changes. Run the big corpus
# only when this is green.
#
#   bench/mini.sh                # both fixtures
#   bench/mini.sh word_count     # one fixture

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SEEKR_BIN="${SEEKR_BIN:-$ROOT/target/release/seekr}"
# 7 minutes: qwen edit turns run 25-35s each, and the budget must leave
# room for the escalation path (guidance/patch) to actually fire — that's
# part of what the mini-bench measures.
BUDGET="${SEEKR_MINI_TIMEOUT:-540}"

if [ "$#" -gt 0 ]; then
  SELECTED=("$@")
else
  SELECTED=(word_count slugify)
fi

pass=0; total=0
for name in "${SELECTED[@]}"; do
  src="$ROOT/bench/mini/$name"
  [ -d "$src" ] || { echo "no such mini fixture: $name" >&2; exit 1; }
  work="$(mktemp -d "/tmp/opencode/mini-$name-XXXXXX")"
  cp -r "$src/." "$work/"
  git -C "$work" init -q
  git -C "$work" add -A
  git -C "$work" -c user.email=bench@seekr -c user.name=seekr-bench \
    commit -qm "mini fixture: $name"

  total=$((total + 1))
  start=$(date +%s)
  echo "── $name (budget ${BUDGET}s)"
  set +e
  (cd "$work" && SEEKR_STATE_DIR="$work/.state" timeout "$BUDGET" \
     "$SEEKR_BIN" run . --plan plan.json --auto --headless) \
     > "$work/run.log" 2>&1
  rc=$?
  set -e
  secs=$(( $(date +%s) - start ))

  if [ $rc -eq 0 ]; then
    pass=$((pass + 1))
    echo "   PASS in ${secs}s"
  else
    echo "   FAIL (rc=$rc) in ${secs}s — tail:"
    tail -6 "$work/run.log" | sed 's/^/     /'
  fi
done

echo
echo "mini-bench: $pass/$total passed"
[ "$pass" -eq "$total" ]
