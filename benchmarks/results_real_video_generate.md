# FastRegex Benchmark Results

- Dataset: `/Users/cesarfavero/Documents/video-generate`
- Indexed docs: 14
- Iterations per query: 3
- no_snippet: true
- Base commit id: `NO_GIT`

| Query | Pattern | Matches | Candidates (p50) | Candidate reduction | FastRegex p50/p95 (ms) | rg p50/p95 (ms) | Full scan PCRE2 p50/p95 (ms) | FastRegex vs rg (p50) |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| function|class | `function|class` | 27 | 9 | 35.7% | 1.48/1.64 | 14.01/16.47 | 4.35/4.46 | 9.45x |
| async function | `async function` | 9 | 6 | 57.1% | 1.07/1.51 | 16.13/18.52 | 4.34/5.17 | 15.02x |
| TODO | `TODO` | 0 | 0 | 100.0% | 0.47/0.74 | 15.37/17.12 | 3.75/4.01 | 32.61x |

Average speedup vs rg (p50 across queries): **19.02x**

Notes:
- Candidate reduction applies to indexed queries and may approach 0% on fallback-heavy regexes.
- All methods were required to return identical match counts in this benchmark run.
