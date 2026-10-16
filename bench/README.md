# Benchmark corpus

Five graded fixtures on the `kv_lsm_bench` template: a Rust lib skeleton
with `todo!()` stubs, committed tests that define done, and a
pre-planned DAG (`plan.json`) so runs measure the worker + gates, not
planner variance.

| Fixture | Task | Steps |
|---|---|---|
| `lru_cache` | bounded LRU: core store + LRU eviction | 2 |
| `csv_fields` | RFC 4180 field parsing + canonical re-encoding | 2 |
| `token_bucket` | rate limiter: refill + take with retry hints | 2 |
| `base32` | RFC 4648 encode + strict decode | 2 |
| `wal_writer` | WAL append with FNV-1a checksums + torn-tail recovery | 2 |

Rules that keep runs honest:

- **Workers may read the tests but never write them** — `allowed_paths`
  confines writes to `src/`, enforced deterministically.
- Fixtures run against **copies** in temp dirs; the corpus is never
  mutated.
- Every completed step passes both the fixture's `cargo test` gate and
  the AiWrangler check gate.

## Run it

```sh
cargo build --release
bench/run.sh                    # all fixtures
bench/run.sh lru_cache base32   # subset
```

Results append to `bench/results.jsonl` (one JSON record per fixture per
run) and print the release-bar report:

- step completion ≥ 80%
- DAG completion ≥ 60%
- frontier calls: 0 completions after planning

## Adding a fixture

1. `bench/fixtures/<name>/` with `Cargo.toml`, `src/lib.rs` (stubs +
   doc-comment spec), and `tests/stepN_tests.rs` per step.
2. A `plan.json` (same schema as `seekr plan` output); step N's
   verification command is `cargo test --test stepN_tests`, the last
   step's is plain `cargo test`.
3. Verify with stubs: tests must **compile** and **fail**.

## Baselines

`bench/results.jsonl` is append-only history; compare across commits
with:

```sh
python3 bench/summarize.py --results bench/results.jsonl
```
