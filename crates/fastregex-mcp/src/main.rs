use std::env;
use std::io::{self, BufRead, BufReader, Write};
use std::path::PathBuf;

use anyhow::{Context, Result, anyhow};
use fastregex_core::{Engine, EngineConfig, RebuildMode, SearchOptions};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Debug, Deserialize)]
struct RpcRequest {
    #[allow(dead_code)]
    jsonrpc: Option<String>,
    id: Option<Value>,
    method: String,
    params: Option<Value>,
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .with_writer(io::stderr)
        .init();

    let (workspace, index_root, auto_index) = parse_args()?;
    let mut config = EngineConfig::for_workspace(&workspace);
    if let Some(index_root) = index_root {
        config.index_root = index_root;
    }

    let engine = Engine::new(config).context("failed to initialize fastregex engine")?;

    if auto_index {
        if let Ok(status) = engine.index_status() {
            if status.freshness != "fresh" {
                let _ = engine.index_rebuild(RebuildMode::Background);
            }
        }
    }

    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut reader = BufReader::new(stdin.lock());
    let mut writer = io::BufWriter::new(stdout.lock());

    while let Some(message) = read_message(&mut reader)? {
        let request: RpcRequest = match serde_json::from_value(message) {
            Ok(req) => req,
            Err(err) => {
                let response = error_response(
                    Value::Null,
                    -32700,
                    format!("invalid json-rpc payload: {err}"),
                );
                write_message(&mut writer, &response)?;
                continue;
            }
        };

        if request.id.is_none() {
            // Notification - best effort handler.
            let _ = handle_notification(&engine, &request);
            continue;
        }

        let id = request.id.clone().unwrap_or(Value::Null);
        let response = match handle_request(&engine, request) {
            Ok(result) => success_response(id, result),
            Err(err) => error_response(id, -32000, err.to_string()),
        };

        write_message(&mut writer, &response)?;
    }

    Ok(())
}

fn parse_args() -> Result<(PathBuf, Option<PathBuf>, bool)> {
    let mut workspace = env::current_dir().context("failed to get current directory")?;
    let mut index_root = None;
    let mut auto_index = true;

    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--workspace" => {
                let value = args.next().context("missing value for --workspace")?;
                workspace = PathBuf::from(value);
            }
            "--index-root" => {
                let value = args.next().context("missing value for --index-root")?;
                index_root = Some(PathBuf::from(value));
            }
            "--auto-index" => auto_index = true,
            "--no-auto-index" => auto_index = false,
            _ => return Err(anyhow!("unknown argument: {arg}")),
        }
    }

    if let Ok(env_value) = env::var("FASTREGEX_AUTO_INDEX") {
        let normalized = env_value.trim().to_ascii_lowercase();
        if matches!(normalized.as_str(), "0" | "false" | "no") {
            auto_index = false;
        }
        if matches!(normalized.as_str(), "1" | "true" | "yes") {
            auto_index = true;
        }
    }

    Ok((workspace, index_root, auto_index))
}

fn handle_notification(engine: &Engine, request: &RpcRequest) -> Result<()> {
    match request.method.as_str() {
        "notifications/initialized" | "initialized" => Ok(()),
        "index_rebuild" => {
            let mode = request
                .params
                .as_ref()
                .and_then(|p| p.get("mode"))
                .and_then(Value::as_str)
                .unwrap_or("background");
            let mode = parse_rebuild_mode(mode)?;
            let _ = engine.index_rebuild(mode)?;
            Ok(())
        }
        _ => Ok(()),
    }
}

fn handle_request(engine: &Engine, request: RpcRequest) -> Result<Value> {
    match request.method.as_str() {
        "initialize" => Ok(json!({
            "protocolVersion": "2025-11-05",
            "capabilities": {
                "tools": {}
            },
            "serverInfo": {
                "name": "fastregex-mcp",
                "version": env!("CARGO_PKG_VERSION")
            }
        })),
        "tools/list" => Ok(json!({
            "tools": tools_manifest()
        })),
        "tools/call" => {
            let params = request
                .params
                .ok_or_else(|| anyhow!("tools/call requires params"))?;
            let name = params
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow!("tools/call requires params.name"))?;
            let args = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));

            let result = dispatch_tool(engine, name, args)?;
            Ok(json!({
                "content": [
                    {
                        "type": "json",
                        "json": result
                    }
                ]
            }))
        }
        // Direct invocation shortcuts.
        "regex_search" => {
            let args = request.params.unwrap_or_else(|| json!({}));
            dispatch_tool(engine, "regex_search", args)
        }
        "index_status" => dispatch_tool(engine, "index_status", json!({})),
        "index_update_files" => {
            let args = request.params.unwrap_or_else(|| json!({}));
            dispatch_tool(engine, "index_update_files", args)
        }
        "index_rebuild" => {
            let args = request.params.unwrap_or_else(|| json!({}));
            dispatch_tool(engine, "index_rebuild", args)
        }
        other => Err(anyhow!("method not found: {other}")),
    }
}

fn dispatch_tool(engine: &Engine, name: &str, args: Value) -> Result<Value> {
    match name {
        "regex_search" => {
            let pattern = args
                .get("pattern")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow!("regex_search requires 'pattern'"))?;

            let options = parse_search_options(&args)?;
            let result = engine.regex_search(pattern, options)?;
            Ok(serde_json::to_value(result)?)
        }
        "index_status" => {
            let result = engine.index_status()?;
            Ok(serde_json::to_value(result)?)
        }
        "index_update_files" => {
            let changed_files = args
                .get("changed_files")
                .and_then(Value::as_array)
                .ok_or_else(|| anyhow!("index_update_files requires 'changed_files' array"))?
                .iter()
                .map(|v| {
                    v.as_str()
                        .map(ToOwned::to_owned)
                        .ok_or_else(|| anyhow!("changed_files must contain only strings"))
                })
                .collect::<Result<Vec<String>>>()?;

            let result = engine.index_update_files(&changed_files)?;
            Ok(serde_json::to_value(result)?)
        }
        "index_rebuild" => {
            let mode_str = args
                .get("mode")
                .and_then(Value::as_str)
                .unwrap_or("background");
            let mode = parse_rebuild_mode(mode_str)?;
            let result = engine.index_rebuild(mode)?;
            Ok(serde_json::to_value(result)?)
        }
        _ => Err(anyhow!("unknown tool '{name}'")),
    }
}

fn parse_search_options(args: &Value) -> Result<SearchOptions> {
    if let Some(options_value) = args.get("options") {
        let options: SearchOptions = serde_json::from_value(options_value.clone())
            .context("invalid regex_search.options payload")?;
        return Ok(options);
    }

    let mut options = SearchOptions::default();

    if let Some(v) = args.get("include") {
        options.include = parse_string_array(v, "include")?;
    }
    if let Some(v) = args.get("exclude") {
        options.exclude = parse_string_array(v, "exclude")?;
    }
    if let Some(v) = args.get("globs") {
        options.globs = parse_string_array(v, "globs")?;
    }
    if let Some(v) = args.get("max_results") {
        options.max_results =
            v.as_u64()
                .ok_or_else(|| anyhow!("max_results must be an integer"))? as usize;
    }
    if let Some(v) = args.get("case_sensitive") {
        options.case_sensitive = v
            .as_bool()
            .ok_or_else(|| anyhow!("case_sensitive must be boolean"))?;
    }
    if let Some(v) = args.get("dotall") {
        options.dotall = v
            .as_bool()
            .ok_or_else(|| anyhow!("dotall must be boolean"))?;
    }
    if let Some(v) = args.get("multiline") {
        options.multiline = v
            .as_bool()
            .ok_or_else(|| anyhow!("multiline must be boolean"))?;
    }
    if let Some(v) = args.get("no_snippet") {
        options.no_snippet = v
            .as_bool()
            .ok_or_else(|| anyhow!("no_snippet must be boolean"))?;
    }
    if let Some(v) = args.get("timeout_ms") {
        options.timeout_ms = Some(
            v.as_u64()
                .ok_or_else(|| anyhow!("timeout_ms must be an integer"))?,
        );
    }
    if let Some(v) = args.get("request_id") {
        options.request_id = Some(
            v.as_str()
                .ok_or_else(|| anyhow!("request_id must be a string"))?
                .to_string(),
        );
    }

    Ok(options)
}

fn parse_string_array(value: &Value, field: &str) -> Result<Vec<String>> {
    let arr = value
        .as_array()
        .ok_or_else(|| anyhow!("{field} must be an array of strings"))?;

    arr.iter()
        .map(|v| {
            v.as_str()
                .map(ToOwned::to_owned)
                .ok_or_else(|| anyhow!("{field} must be an array of strings"))
        })
        .collect()
}

fn parse_rebuild_mode(raw: &str) -> Result<RebuildMode> {
    match raw {
        "foreground" => Ok(RebuildMode::Foreground),
        "background" => Ok(RebuildMode::Background),
        _ => Err(anyhow!("invalid rebuild mode '{raw}'")),
    }
}

fn tools_manifest() -> Vec<Value> {
    vec![
        json!({
            "name": "regex_search",
            "description": "Search files with PCRE2 final verification using fast indexed candidate selection.",
            "inputSchema": {
                "type": "object",
                "required": ["pattern"],
                "properties": {
                    "pattern": { "type": "string" },
                    "options": { "type": "object" }
                }
            }
        }),
        json!({
            "name": "index_status",
            "description": "Return base commit, freshness and overlay status.",
            "inputSchema": {
                "type": "object",
                "properties": {}
            }
        }),
        json!({
            "name": "index_update_files",
            "description": "Update overlay mini-index for changed files.",
            "inputSchema": {
                "type": "object",
                "required": ["changed_files"],
                "properties": {
                    "changed_files": {
                        "type": "array",
                        "items": { "type": "string" }
                    }
                }
            }
        }),
        json!({
            "name": "index_rebuild",
            "description": "Rebuild index in foreground or background.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "mode": {
                        "type": "string",
                        "enum": ["foreground", "background"]
                    }
                }
            }
        }),
    ]
}

fn read_message<R: BufRead>(reader: &mut R) -> Result<Option<Value>> {
    let mut content_length: Option<usize> = None;

    loop {
        let mut line = String::new();
        let bytes_read = reader.read_line(&mut line)?;

        if bytes_read == 0 {
            return Ok(None);
        }

        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }

        if let Some((name, value)) = trimmed.split_once(':') {
            if name.eq_ignore_ascii_case("Content-Length") {
                content_length = Some(
                    value
                        .trim()
                        .parse::<usize>()
                        .context("invalid Content-Length value")?,
                );
            }
        }
    }

    let content_length = content_length.ok_or_else(|| anyhow!("missing Content-Length header"))?;
    let mut payload = vec![0u8; content_length];
    reader.read_exact(&mut payload)?;

    let value: Value = serde_json::from_slice(&payload).context("invalid JSON payload")?;
    Ok(Some(value))
}

fn write_message<W: Write>(writer: &mut W, value: &Value) -> Result<()> {
    let body = serde_json::to_vec(value)?;
    write!(writer, "Content-Length: {}\r\n\r\n", body.len())?;
    writer.write_all(&body)?;
    writer.flush()?;
    Ok(())
}

fn success_response(id: Value, result: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result
    })
}

fn error_response(id: Value, code: i64, message: String) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": code,
            "message": message
        }
    })
}
