# FastRegex Benchmark Results

- Dataset: `benchmarks/synthetic_3000`
- Generated files: 3000
- Iterations per query: 3
- no_snippet: true
- Base commit id: `5c1d9e9e0def137eeeb76a3600d29c123da5a130`

| Query | Pattern | Matches | Candidates (p50) | Candidate reduction | FastRegex p50/p95 (ms) | rg p50/p95 (ms) | Full scan PCRE2 p50/p95 (ms) | FastRegex vs rg (p50) |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| Literal token | `needle_alpha_beta` | 429 | 429 | 85.7% | 28.85/29.20 | 54.09/57.84 | 77.55/82.19 | 1.87x |
| Alternation literal | `alpha_service_endpoint|gamma_worker_token` | 504 | 483 | 83.9% | 33.23/35.05 | 53.85/61.17 | 83.82/85.17 | 1.62x |
| Class-heavy (indexed) | `user_[0-9]{4}_event_[A-Z]{3}` | 131 | 131 | 95.6% | 8.52/8.54 | 36.96/41.47 | 60.93/62.07 | 4.34x |
| Hex digest (fallback) | `[a-f0-9]{40}` | 177 | 3001 | 0.0% | 278.92/297.51 | 48.61/49.42 | 171.63/204.77 | 0.17x |

Average speedup vs rg (p50 across queries): **2.00x**

Notes:
- Candidate reduction applies to indexed queries and may approach 0% on fallback-heavy regexes.
- All methods were required to return identical match counts in this benchmark run.
