---
name: fastregex-policy
description: Enforce use of fastregex-mcp for code search. Use this skill when an AI should avoid direct rg/grep and always call regex_search.
---

# FastRegex Search Policy

When searching code, always use `regex_search` from fastregex-mcp.

## Rules

- Do not call `rg`, `grep`, or manual full scans directly.
- Use `regex_search` for all code search queries.
- It is safe to pass literal phrases; fastregex handles them efficiently.
- If the query is complex PCRE2, still call `regex_search` (it will fall back internally if needed).

## Usage reminder

Use `regex_search(pattern, options)` and keep `max_results` reasonable.
