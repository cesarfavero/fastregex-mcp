# FastRegex Benchmark Results

- Dataset: `benchmarks/synthetic_96000`
- Generated files: 96000
- Iterations per query: 1
- Base commit id: `f337b8bb7e877666cc99cb7cfc3e2842bd55efe8`

| Query | Pattern | Matches | Candidates (p50) | Candidate reduction | FastRegex p50/p95 (ms) | rg p50/p95 (ms) | Full scan PCRE2 p50/p95 (ms) | FastRegex vs rg (p50) |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| Literal token | `needle_alpha_beta` | 13715 | 13715 | 85.7% | 2618.16/2618.16 | 2487.28/2487.28 | 6587.69/6587.69 | 0.95x |
| Alternation literal | `alpha_service_endpoint|gamma_worker_token` | 16113 | 15441 | 83.9% | 2631.77/2631.77 | 2117.23/2117.23 | 6616.37/6616.37 | 0.80x |
| Class-heavy (indexed) | `user_[0-9]{4}_event_[A-Z]{3}` | 4174 | 4174 | 95.7% | 755.60/755.60 | 1739.02/1739.02 | 5911.90/5911.90 | 2.30x |
| Hex digest (fallback) | `[a-f0-9]{40}` | 5648 | 96001 | 0.0% | 12631.94/12631.94 | 1877.35/1877.35 | 9092.20/9092.20 | 0.15x |

Average speedup vs rg (p50 across queries): **1.05x**

Notes:
- Candidate reduction applies to indexed queries and may approach 0% on fallback-heavy regexes.
- All methods were required to return identical match counts in this benchmark run.
