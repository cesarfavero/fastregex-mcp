---
name: fastregex-install
description: Install and run fastregex-mcp locally for a given repo (path or git URL). Use this skill when the user asks the AI to install fastregex or enable indexed search in their project.
---

# FastRegex MCP Install (Local)

Use this workflow when a user asks to install or enable FastRegex in a repo.

## Inputs to collect

- **Repo source**: local path or git URL.
- **Run mode**: local dev (cargo run) or background service.

## Workflow

1. **Get the repo**
   - If the user provided a git URL, clone it:
     ```bash
     git clone <repo_url>
     cd <repo_dir>
     ```
   - If a local path is provided, `cd` there.

2. **Get fastregex-mcp**
   - Option A (recommended for testing): clone next to the repo:
     ```bash
     git clone https://github.com/cesarfavero/fastregex-mcp.git .fastregex-mcp
     ```
   - Option B: clone elsewhere and reference it directly.

3. **Run the MCP**
   - From the fastregex-mcp folder:
     ```bash
     cargo run -p fastregex-mcp -- --workspace /path/to/repo
     ```
   - The MCP auto-indexes on startup. It logs whether the index is ready or rebuilding.

4. **Verify** (optional)
   - Send a JSON-RPC `initialize` call or run a quick `regex_search`.

5. **Usage policy**
   - Instruct the model to always call `regex_search` for code search and avoid direct `rg`.

## Notes

- Indexing is local. No code is uploaded.
- `regex_search` is exact and fast for literal phrases.
- For background use, keep the process running (systemd/tmux/etc.).
