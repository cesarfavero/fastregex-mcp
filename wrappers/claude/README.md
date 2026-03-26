# FastRegex MCP Wrapper (Claude Desktop)

Configure Claude Desktop MCP to launch `fastregex-mcp` and expose only:

- `regex_search`
- `index_status`
- `index_update_files`
- `index_rebuild`

Policy recommendation for prompt/system instructions:

> Always search code using `regex_search` from fastregex-mcp. Do not execute direct grep/ripgrep commands unless explicitly requested for diagnostics.
