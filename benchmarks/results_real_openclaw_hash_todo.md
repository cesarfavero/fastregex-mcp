# FastRegex Benchmark Results

- Dataset: `/Users/cesarfavero/Documents/openclaw`
- Indexed docs: 10550
- Iterations per query: 7
- no_snippet: true
- Base commit id: `NO_GIT`

| Query | Pattern | Matches | Candidates (p50) | Candidate reduction | FastRegex p50/p95 (ms) | rg p50/p95 (ms) | Full scan PCRE2 p50/p95 (ms) | FastRegex vs rg (p50) |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| hash_literal | `TODO` | 63 | 21 | 99.8% | 68.06/71.00 | 273.12/284.38 | 150.96/168.65 | 4.01x |

Average speedup vs rg (p50 across queries): **4.01x**

Notes:
- Candidate reduction applies to indexed queries and may approach 0% on fallback-heavy regexes.
- All methods were required to return identical match counts in this benchmark run.
