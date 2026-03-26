# FastRegex Benchmark Results

- Dataset: `benchmarks/synthetic_24000`
- Generated files: 24000
- Iterations per query: 3
- Base commit id: `f337b8bb7e877666cc99cb7cfc3e2842bd55efe8`

| Query | Pattern | Matches | Candidates (p50) | Candidate reduction | FastRegex p50/p95 (ms) | rg p50/p95 (ms) | Full scan PCRE2 p50/p95 (ms) | FastRegex vs rg (p50) |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| Literal token | `needle_alpha_beta` | 3429 | 3429 | 85.7% | 248.91/284.44 | 396.55/420.54 | 1062.39/1094.34 | 1.59x |
| Alternation literal | `alpha_service_endpoint|gamma_worker_token` | 4029 | 3861 | 83.9% | 219.71/220.29 | 334.19/355.09 | 725.14/890.29 | 1.52x |
| Class-heavy (indexed) | `user_[0-9]{4}_event_[A-Z]{3}` | 1044 | 1044 | 95.7% | 51.67/51.94 | 221.40/280.20 | 420.65/426.24 | 4.29x |
| Hex digest (fallback) | `[a-f0-9]{40}` | 1412 | 24001 | 0.0% | 1886.22/1968.79 | 239.61/245.39 | 1153.83/1189.36 | 0.13x |

Average speedup vs rg (p50 across queries): **1.88x**

Notes:
- Candidate reduction applies to indexed queries and may approach 0% on fallback-heavy regexes.
- All methods were required to return identical match counts in this benchmark run.
