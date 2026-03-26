# FastRegex Benchmark Results

- Dataset: `benchmarks/synthetic_monorepo`
- Generated files: 6000
- Iterations per query: 9
- Base commit id: `NO_GIT`

| Query | Pattern | Matches | Candidates (p50) | Candidate reduction | FastRegex p50/p95 (ms) | rg p50/p95 (ms) | Full scan PCRE2 p50/p95 (ms) | FastRegex vs rg (p50) |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| Literal token | `needle_alpha_beta` | 858 | 858 | 85.7% | 34.51/39.47 | 73.16/102.40 | 130.11/141.10 | 2.12x |
| Alternation literal | `alpha_service_endpoint|gamma_worker_token` | 1008 | 966 | 83.9% | 42.85/46.63 | 75.00/87.82 | 138.19/270.26 | 1.75x |
| Class-heavy (fallback) | `user_[0-9]{4}_event_[A-Z]{3}` | 261 | 6001 | 0.0% | 104.75/107.06 | 53.46/61.38 | 93.34/99.83 | 0.51x |
| Hex digest (fallback) | `[a-f0-9]{40}` | 353 | 6001 | 0.0% | 287.16/388.80 | 55.87/66.99 | 280.20/285.69 | 0.19x |

Average speedup vs rg (p50 across queries): **1.14x**

Notes:
- Candidate reduction applies to indexed queries and may approach 0% on fallback-heavy regexes.
- All methods were required to return identical match counts in this benchmark run.