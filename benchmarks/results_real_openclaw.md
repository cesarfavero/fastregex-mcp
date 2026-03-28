# FastRegex Benchmark Results

- Dataset: `/Users/cesarfavero/Documents/openclaw`
- Indexed docs: 10550
- Iterations per query: 7
- no_snippet: true
- Base commit id: `NO_GIT`

| Query | Pattern | Matches | Candidates (p50) | Candidate reduction | FastRegex p50/p95 (ms) | rg p50/p95 (ms) | Full scan PCRE2 p50/p95 (ms) | FastRegex vs rg (p50) |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| TODO | `TODO` | 63 | 21 | 99.8% | 68.30/93.69 | 283.54/330.11 | 161.13/231.47 | 4.15x |

Average speedup vs rg (p50 across queries): **4.15x**

Notes:
- Candidate reduction applies to indexed queries and may approach 0% on fallback-heavy regexes.
- All methods were required to return identical match counts in this benchmark run.
