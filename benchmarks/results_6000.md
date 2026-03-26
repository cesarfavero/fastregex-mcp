# FastRegex Benchmark Results

- Dataset: `benchmarks/synthetic_6000`
- Generated files: 6000
- Iterations per query: 9
- Base commit id: `f337b8bb7e877666cc99cb7cfc3e2842bd55efe8`

| Query | Pattern | Matches | Candidates (p50) | Candidate reduction | FastRegex p50/p95 (ms) | rg p50/p95 (ms) | Full scan PCRE2 p50/p95 (ms) | FastRegex vs rg (p50) |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| Literal token | `needle_alpha_beta` | 858 | 858 | 85.7% | 45.83/47.48 | 71.09/75.39 | 126.71/135.25 | 1.55x |
| Alternation literal | `alpha_service_endpoint|gamma_worker_token` | 1008 | 966 | 83.9% | 54.51/58.74 | 76.21/126.92 | 138.00/155.80 | 1.40x |
| Class-heavy (indexed) | `user_[0-9]{4}_event_[A-Z]{3}` | 261 | 261 | 95.7% | 13.25/15.74 | 54.47/56.09 | 94.66/101.62 | 4.11x |
| Hex digest (fallback) | `[a-f0-9]{40}` | 353 | 6001 | 0.0% | 481.38/535.20 | 61.26/65.66 | 280.74/302.14 | 0.13x |

Average speedup vs rg (p50 across queries): **1.80x**

Notes:
- Candidate reduction applies to indexed queries and may approach 0% on fallback-heavy regexes.
- All methods were required to return identical match counts in this benchmark run.
