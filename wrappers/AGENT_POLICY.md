# Agent Search Policy

## Default behavior

- Always use `regex_search` from `fastregex-mcp` as the primary search tool.
- Do not call raw `grep`/`rg` from the model layer for normal repository search.

## Freshness flow

1. Call `index_status`.
2. Search with `regex_search`.
3. After edits, call `index_update_files` with changed paths.
4. Re-run `regex_search`.

## Rebuild

- Use `index_rebuild` in `background` mode by default.
- Use `foreground` only for explicit maintenance/debug sessions.
