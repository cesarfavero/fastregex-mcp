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

Auto-indexing on startup (default):

- On startup, the MCP checks index freshness and triggers a background rebuild if stale.
- Disable with `--no-auto-index` or `FASTREGEX_AUTO_INDEX=0`.

## Install / Use on a repo (local)

What happens:

- The MCP is installed and run locally by the user (desktop or VPS).
- It indexes the repo pointed by `--workspace`.
- The client/IA calls `regex_search` and gets responses immediately from the index.

Quick start (from this repo):

```bash
# 1) Clone fastregex-mcp
git clone https://github.com/cesarfavero/fastregex-mcp.git
cd fastregex-mcp

# 2) Run MCP pointing to a target repo
cargo run -p fastregex-mcp -- --workspace /path/to/your-repo
```

If you want to install it inside the target repo folder:

```bash
cd /path/to/your-repo
git clone https://github.com/cesarfavero/fastregex-mcp.git .fastregex-mcp
cd .fastregex-mcp
cargo run -p fastregex-mcp -- --workspace /path/to/your-repo
```

Notes:

- This does not upload code anywhere. Indexing is local.
- `regex_search` handles literal phrases automatically and is fast by default.

## AI install guide (when user sends a repo link)

Use this checklist when an AI agent is asked to install FastRegex for a repo:

1. Clone the target repo (if a URL was provided).
2. Clone `fastregex-mcp` locally (side-by-side or inside the repo as `.fastregex-mcp`).
3. Run the MCP pointing at the repo path.
4. Instruct the model to use `regex_search` for all code search.

Skill file (for Smithery or similar tools):

- `wrappers/skills/fastregex-install/SKILL.md`
- `wrappers/skills/fastregex-policy/SKILL.md`

Smithery example:

```bash
npx @smithery/cli@latest skill add fastregex-install - < wrappers/skills/fastregex-install/SKILL.md
npx @smithery/cli@latest skill add fastregex-policy - < wrappers/skills/fastregex-policy/SKILL.md
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

## Benchmark (real runs, March 26, 2026)

Environment:

- macOS 26.0.1 (build 25A362)
- Apple M1, 8 GB RAM
- engine: this repository at the commit recorded in each report header

### Run A: 6000 files, 9 iterations/query

| Query | Pattern | Matches | Candidates (p50) | Candidate reduction | FastRegex p50/p95 (ms) | rg p50/p95 (ms) | Full scan PCRE2 p50/p95 (ms) | FastRegex vs rg (p50) |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| Literal token | `needle_alpha_beta` | 858 | 858 | 85.7% | 45.83/47.48 | 71.09/75.39 | 126.71/135.25 | 1.55x |
| Alternation literal | `alpha_service_endpoint|gamma_worker_token` | 1008 | 966 | 83.9% | 54.51/58.74 | 76.21/126.92 | 138.00/155.80 | 1.40x |
| Class-heavy (indexed) | `user_[0-9]{4}_event_[A-Z]{3}` | 261 | 261 | 95.7% | 13.25/15.74 | 54.47/56.09 | 94.66/101.62 | 4.11x |
| Hex digest (fallback) | `[a-f0-9]{40}` | 353 | 6001 | 0.0% | 481.38/535.20 | 61.26/65.66 | 280.74/302.14 | 0.13x |

Average speedup vs `rg` (p50 across these 4 queries): **1.80x**.

### Run B: 24000 files, 3 iterations/query

| Query | Pattern | Matches | Candidates (p50) | Candidate reduction | FastRegex p50/p95 (ms) | rg p50/p95 (ms) | Full scan PCRE2 p50/p95 (ms) | FastRegex vs rg (p50) |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| Literal token | `needle_alpha_beta` | 3429 | 3429 | 85.7% | 248.91/284.44 | 396.55/420.54 | 1062.39/1094.34 | 1.59x |
| Alternation literal | `alpha_service_endpoint|gamma_worker_token` | 4029 | 3861 | 83.9% | 219.71/220.29 | 334.19/355.09 | 725.14/890.29 | 1.52x |
| Class-heavy (indexed) | `user_[0-9]{4}_event_[A-Z]{3}` | 1044 | 1044 | 95.7% | 51.67/51.94 | 221.40/280.20 | 420.65/426.24 | 4.29x |
| Hex digest (fallback) | `[a-f0-9]{40}` | 1412 | 24001 | 0.0% | 1886.22/1968.79 | 239.61/245.39 | 1153.83/1189.36 | 0.13x |

Average speedup vs `rg` (p50 across these 4 queries): **1.88x**.

### Run C: 96000 files, 1 iteration/query

| Query | Pattern | Matches | Candidates (p50) | Candidate reduction | FastRegex p50/p95 (ms) | rg p50/p95 (ms) | Full scan PCRE2 p50/p95 (ms) | FastRegex vs rg (p50) |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| Literal token | `needle_alpha_beta` | 13715 | 13715 | 85.7% | 2618.16/2618.16 | 2487.28/2487.28 | 6587.69/6587.69 | 0.95x |
| Alternation literal | `alpha_service_endpoint|gamma_worker_token` | 16113 | 15441 | 83.9% | 2631.77/2631.77 | 2117.23/2117.23 | 6616.37/6616.37 | 0.80x |
| Class-heavy (indexed) | `user_[0-9]{4}_event_[A-Z]{3}` | 4174 | 4174 | 95.7% | 755.60/755.60 | 1739.02/1739.02 | 5911.90/5911.90 | 2.30x |
| Hex digest (fallback) | `[a-f0-9]{40}` | 5648 | 96001 | 0.0% | 12631.94/12631.94 | 1877.35/1877.35 | 9092.20/9092.20 | 0.15x |

Average speedup vs `rg` (p50 across these 4 queries): **1.05x**.
Note: Run C is a single-iteration sample and should be treated as directional.

### Quick tuning run (3000 files, no_snippet=true)

- Average speedup vs `rg`: **2.00x**
- Full report: `benchmarks/results_3000.md`

### Reality check on "millions of files in milliseconds"

- With selective patterns (indexed literals), the approach is clearly faster than `rg` in these runs.
- With low-selectivity patterns like `[a-f0-9]{40}` (no useful literals), V1 correctly falls back to near full-scan behavior and is slower than `rg`.
- At 96k files, indexed queries are in seconds, not milliseconds. So this V1 is **not** a universal "milliseconds for any regex" claim yet; it is fast when the planner can extract selective constraints and the candidate set is small.

## Reproduce benchmark

```bash
cargo run -p fastregex-bench -- --dataset benchmarks/synthetic_6000 --files 6000 --iterations 9
cargo run -p fastregex-bench -- --dataset benchmarks/synthetic_24000 --files 24000 --iterations 3
cargo run -p fastregex-bench -- --dataset benchmarks/synthetic_96000 --files 96000 --iterations 1
```

Generated outputs:

- report: `benchmarks/latest-results.md`
- saved snapshots: `benchmarks/results_6000.md`, `benchmarks/results_24000.md`, `benchmarks/results_96000.md`
- datasets (ignored by git): `benchmarks/synthetic_*`

Real repo benchmark:

```bash
cargo run -p fastregex-bench -- --real /path/to/repo --iterations 3 --pattern "function|class" --pattern "TODO"
```

Example (video-generate):

- Report: `benchmarks/results_real_video_generate.md`
- Average speedup vs `rg`: **19.02x** (3 patterns, no_snippet=true)
- TODO pattern speedup: **32.61x** (best-case literal)
