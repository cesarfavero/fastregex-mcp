# FastRegex Benchmark Results

- Dataset: `/Users/cesarfavero/Documents/openclaw`
- Indexed docs: 10550
- Iterations per query: 1
- no_snippet: true
- Base commit id: `NO_GIT`

| Query | Pattern | Matches | Candidates (p50) | Candidate reduction | FastRegex p50/p95 (ms) | rg p50/p95 (ms) | Full scan PCRE2 p50/p95 (ms) | FastRegex vs rg (p50) |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| function|class | `function|class` | 45143 | 6199 | 41.2% | 945.25/945.25 | 838.64/838.64 | 380.89/380.89 | 0.89x |
| async function | `async function` | 5942 | 3195 | 69.7% | 476.91/476.91 | 192.86/192.86 | 277.59/277.59 | 0.40x |
| TODO | `TODO` | 63 | 21 | 99.8% | 71.26/71.26 | 279.85/279.85 | 147.83/147.83 | 3.93x |

Average speedup vs rg (p50 across queries): **1.74x**

Notes:
- Candidate reduction applies to indexed queries and may approach 0% on fallback-heavy regexes.
- All methods were required to return identical match counts in this benchmark run.
