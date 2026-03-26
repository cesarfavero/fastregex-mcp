# fastregex-mcp

Fast Regex Search Universal V1 (MCP-first, Rust, PCRE2) with local commit-anchored indexing and incremental overlay freshness.

Projeto idealizado e liderado por **Cesar Favero**.

## Why this exists

Agent loops often call `rg` repeatedly and full-scan large monorepos. This project replaces that default with an indexed regex search path:

1. Build local index (`postings.bin` + `lookup.bin`) per commit.
2. Extract trigrams + sparse n-grams from query literals.
3. Reduce candidate docs through posting list combination.
4. Run final PCRE2 verification only on candidates.

Design reference (same direction, local-first): [Cursor blog: Fast regex search](https://cursor.com/blog/fast-regex-search).

## Workspace layout

- `crates/fastregex-core`: index builder, sparse/trigram extraction, planner, overlay merge, PCRE2 final matcher.
- `crates/fastregex-mcp`: MCP server over stdio exposing tool API.
- `crates/fastregex-bench`: reproducible benchmark runner (`fastregex` vs `rg` vs full PCRE2 scan).
- `wrappers/`: host integration guidance and search-policy docs.

## Build

```bash
cargo build --workspace
```

## Run MCP server

```bash
cargo run -p fastregex-mcp -- --workspace /path/to/repo
```

Optional custom index path:

```bash
cargo run -p fastregex-mcp -- --workspace /path/to/repo --index-root /tmp/fastregex-index
```

## MCP API

- `regex_search(pattern, options)`
- `index_status()`
- `index_update_files(changed_files[])`
- `index_rebuild(mode)` where `mode` is `foreground` or `background`

## Correctness guarantees

- Final match engine is always **PCRE2**.
- Candidate filtering allows false positives.
- For indexed paths, no false negatives are expected in final results.
- If planner extraction is not safe/useful, it falls back to full-candidate scan internally.

## Benchmark (real run, March 26, 2026)

Environment:

- macOS 26.0.1 (build 25A362)
- Apple M1, 8 GB RAM
- dataset: synthetic monorepo with 6000 files
- iterations: 9 per query

| Query | Pattern | Matches | Candidates (p50) | Candidate reduction | FastRegex p50/p95 (ms) | rg p50/p95 (ms) | Full scan PCRE2 p50/p95 (ms) | FastRegex vs rg (p50) |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| Literal token | `needle_alpha_beta` | 858 | 858 | 85.7% | 34.51/39.47 | 73.16/102.40 | 130.11/141.10 | 2.12x |
| Alternation literal | `alpha_service_endpoint|gamma_worker_token` | 1008 | 966 | 83.9% | 42.85/46.63 | 75.00/87.82 | 138.19/270.26 | 1.75x |
| Class-heavy (fallback) | `user_[0-9]{4}_event_[A-Z]{3}` | 261 | 6001 | 0.0% | 104.75/107.06 | 53.46/61.38 | 93.34/99.83 | 0.51x |
| Hex digest (fallback) | `[a-f0-9]{40}` | 353 | 6001 | 0.0% | 287.16/388.80 | 55.87/66.99 | 280.20/285.69 | 0.19x |

Average speedup vs `rg` (p50 across these 4 queries): **1.14x**.

Interpretation:

- Indexed literal-heavy queries are materially faster than `rg` in this run.
- Fallback-heavy regexes (little/no extractable literals) still work correctly, but currently lose to `rg` in latency.
- This matches expected behavior for V1 and points to the next optimization targets.

## Reproduce benchmark

```bash
cargo run -p fastregex-bench -- --files 6000 --iterations 9
```

Generated outputs:

- report: `benchmarks/latest-results.md`
- dataset (ignored by git): `benchmarks/synthetic_monorepo`
