# FastRegex Benchmark Results

- Dataset: `/Users/cesarfavero/Documents/openclaw`
- Indexed docs: 10550
- Iterations per query: 1
- no_snippet: true
- Base commit id: `NO_GIT`

| Query | Pattern | Matches | Candidates (p50) | Candidate reduction | FastRegex p50/p95 (ms) | rg p50/p95 (ms) | Full scan PCRE2 p50/p95 (ms) | FastRegex vs rg (p50) |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| hash_literal | `TODO` | 63 | 21 | 99.8% | 74.01/74.01 | 426.02/426.02 | 722.04/722.04 | 5.76x |

Average speedup vs rg (p50 across queries): **5.76x**

Notes:
- Candidate reduction applies to indexed queries and may approach 0% on fallback-heavy regexes.
- All methods were required to return identical match counts in this benchmark run.
