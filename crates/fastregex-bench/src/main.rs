use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use fastregex_core::{Engine, EngineConfig, SearchOptions};
use pcre2::bytes::RegexBuilder;
use serde_json::Value;
use walkdir::WalkDir;

#[derive(Debug, Clone)]
struct Config {
    dataset_root: PathBuf,
    files: usize,
    iterations: usize,
    prepare_only: bool,
}

#[derive(Debug, Clone)]
struct QueryCase {
    name: &'static str,
    pattern: &'static str,
}

#[derive(Debug, Clone)]
struct QueryResult {
    name: String,
    pattern: String,
    match_count: usize,
    candidate_count: usize,
    total_docs: usize,
    fast_p50_ms: f64,
    fast_p95_ms: f64,
    rg_p50_ms: f64,
    rg_p95_ms: f64,
    full_scan_p50_ms: f64,
    full_scan_p95_ms: f64,
}

fn main() -> Result<()> {
    let cfg = parse_args()?;

    ensure_synthetic_dataset(&cfg.dataset_root, cfg.files)?;
    if cfg.prepare_only {
        println!(
            "dataset prepared at {} with {} files",
            cfg.dataset_root.display(),
            cfg.files
        );
        return Ok(());
    }

    let mut engine_cfg = EngineConfig::for_workspace(&cfg.dataset_root);
    engine_cfg.build.max_file_bytes = 8 * 1024 * 1024;

    let engine = Engine::new(engine_cfg)?;
    let status = engine.index_status()?;

    let cases = vec![
        QueryCase {
            name: "Literal token",
            pattern: "needle_alpha_beta",
        },
        QueryCase {
            name: "Alternation literal",
            pattern: "alpha_service_endpoint|gamma_worker_token",
        },
        QueryCase {
            name: "Class-heavy (indexed)",
            pattern: "user_[0-9]{4}_event_[A-Z]{3}",
        },
        QueryCase {
            name: "Hex digest (fallback)",
            pattern: "[a-f0-9]{40}",
        },
    ];

    let file_list = collect_text_files(&cfg.dataset_root)?;

    let mut query_results = Vec::new();

    for case in &cases {
        let result = benchmark_case(
            &engine,
            &cfg.dataset_root,
            &file_list,
            status.indexed_docs,
            case,
            cfg.iterations,
        )?;
        query_results.push(result);
    }

    let report = render_markdown_report(&cfg, &status.base_commit, &query_results);
    fs::create_dir_all("benchmarks")?;
    fs::write("benchmarks/latest-results.md", &report)?;

    println!("{report}");
    Ok(())
}

fn parse_args() -> Result<Config> {
    let mut cfg = Config {
        dataset_root: PathBuf::from("benchmarks/synthetic_monorepo"),
        files: 6000,
        iterations: 9,
        prepare_only: false,
    };

    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--dataset" => {
                let value = args.next().context("missing value for --dataset")?;
                cfg.dataset_root = PathBuf::from(value);
            }
            "--files" => {
                let value = args.next().context("missing value for --files")?;
                cfg.files = value.parse().context("--files must be an integer")?;
            }
            "--iterations" => {
                let value = args.next().context("missing value for --iterations")?;
                cfg.iterations = value.parse().context("--iterations must be an integer")?;
            }
            "--prepare-only" => cfg.prepare_only = true,
            other => return Err(anyhow!("unknown argument: {other}")),
        }
    }

    if cfg.files == 0 {
        return Err(anyhow!("--files must be > 0"));
    }

    if cfg.iterations == 0 {
        return Err(anyhow!("--iterations must be > 0"));
    }

    Ok(cfg)
}

fn ensure_synthetic_dataset(root: &Path, files: usize) -> Result<()> {
    let marker_path = root.join(".fastregex-bench.json");

    if marker_path.exists() {
        let marker_raw = fs::read_to_string(&marker_path)?;
        let marker: Value = serde_json::from_str(&marker_raw)?;
        let existing = marker
            .get("files")
            .and_then(Value::as_u64)
            .unwrap_or_default() as usize;
        if existing == files {
            return Ok(());
        }

        fs::remove_dir_all(root)?;
    }

    fs::create_dir_all(root)?;

    for idx in 0..files {
        let pkg = idx / 30;
        let file = idx % 30;
        let ext = match idx % 3 {
            0 => "ts",
            1 => "rs",
            _ => "md",
        };

        let path = root
            .join("packages")
            .join(format!("pkg_{pkg:04}"))
            .join("src")
            .join(format!("module_{file:02}.{ext}"));

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let mut out = String::new();
        out.push_str("// fastregex synthetic benchmark corpus\n");
        out.push_str(&format!("// file_index={idx}\n\n"));

        for line in 0..80usize {
            let w1 = dictionary((idx * 13 + line * 3) % DICTIONARY.len());
            let w2 = dictionary((idx * 7 + line * 11) % DICTIONARY.len());
            let w3 = dictionary((idx * 17 + line * 5) % DICTIONARY.len());
            out.push_str(&format!(
                "const entry_{line:03} = \"{w1}_{w2}_{w3}_{}_{}\";\n",
                idx % 1000,
                line
            ));
        }

        if idx % 7 == 0 {
            out.push_str("const FEATURE_FLAG = \"needle_alpha_beta\";\n");
        }

        if idx % 11 == 0 {
            out.push_str("const API_NAME = \"alpha_service_endpoint\";\n");
        }

        if idx % 13 == 0 {
            out.push_str("const API_ALT = \"gamma_worker_token\";\n");
        }

        if idx % 17 == 0 {
            out.push_str(&format!("// digest: {:040x}\n", idx as u128 * 97_531));
        }

        if idx % 23 == 0 {
            let code = [
                ((idx / 23) % 26) as u8 + b'A',
                ((idx / 13) % 26) as u8 + b'A',
                ((idx / 7) % 26) as u8 + b'A',
            ];
            let code = String::from_utf8_lossy(&code).to_string();
            out.push_str(&format!(
                "let event = \"user_{:04}_event_{code}\";\n",
                idx % 10000
            ));
        }

        let mut file = fs::File::create(path)?;
        file.write_all(out.as_bytes())?;
    }

    let marker = serde_json::json!({
        "dataset": "synthetic_monorepo",
        "files": files,
        "format": 1,
    });
    fs::write(marker_path, serde_json::to_string_pretty(&marker)?)?;

    Ok(())
}

const DICTIONARY: &[&str] = &[
    "engine",
    "planner",
    "posting",
    "lookup",
    "overlay",
    "commit",
    "workspace",
    "search",
    "literal",
    "sparse",
    "trigram",
    "candidate",
    "regex",
    "parser",
    "builder",
    "reader",
    "writer",
    "offset",
    "binary",
    "mmap",
    "latency",
    "throughput",
    "result",
    "snippet",
    "timeout",
    "request",
    "response",
    "module",
    "package",
    "service",
    "storage",
    "queue",
];

fn dictionary(i: usize) -> &'static str {
    DICTIONARY[i % DICTIONARY.len()]
}

fn benchmark_case(
    engine: &Engine,
    dataset_root: &Path,
    files: &[PathBuf],
    total_docs: usize,
    case: &QueryCase,
    iterations: usize,
) -> Result<QueryResult> {
    let mut options = SearchOptions::default();
    options.max_results = 250_000;

    // Warm-up.
    let warm = engine.regex_search(case.pattern, options.clone())?;
    let expected_matches = warm.matches.len();

    let _ = run_ripgrep(dataset_root, case.pattern)?;
    let _ = run_full_scan(files, case.pattern)?;

    let mut fast_times = Vec::<f64>::new();
    let mut rg_times = Vec::<f64>::new();
    let mut full_times = Vec::<f64>::new();
    let mut candidate_counts = Vec::<usize>::new();

    for _ in 0..iterations {
        let t0 = Instant::now();
        let fast = engine.regex_search(case.pattern, options.clone())?;
        fast_times.push(elapsed_ms(t0.elapsed()));
        candidate_counts.push(fast.candidate_count);

        if fast.matches.len() != expected_matches {
            return Err(anyhow!(
                "inconsistent fastregex match count for pattern '{}'",
                case.pattern
            ));
        }

        let t1 = Instant::now();
        let rg_count = run_ripgrep(dataset_root, case.pattern)?;
        rg_times.push(elapsed_ms(t1.elapsed()));

        if rg_count != expected_matches {
            return Err(anyhow!(
                "ripgrep count mismatch for pattern '{}': expected {}, got {}",
                case.pattern,
                expected_matches,
                rg_count
            ));
        }

        let t2 = Instant::now();
        let full_count = run_full_scan(files, case.pattern)?;
        full_times.push(elapsed_ms(t2.elapsed()));

        if full_count != expected_matches {
            return Err(anyhow!(
                "full-scan count mismatch for pattern '{}': expected {}, got {}",
                case.pattern,
                expected_matches,
                full_count
            ));
        }
    }

    let candidate_count = percentile_usize(candidate_counts, 0.5);

    Ok(QueryResult {
        name: case.name.to_string(),
        pattern: case.pattern.to_string(),
        match_count: expected_matches,
        candidate_count,
        total_docs,
        fast_p50_ms: percentile(fast_times.clone(), 0.5),
        fast_p95_ms: percentile(fast_times, 0.95),
        rg_p50_ms: percentile(rg_times.clone(), 0.5),
        rg_p95_ms: percentile(rg_times, 0.95),
        full_scan_p50_ms: percentile(full_times.clone(), 0.5),
        full_scan_p95_ms: percentile(full_times, 0.95),
    })
}

fn collect_text_files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for entry in WalkDir::new(root)
        .into_iter()
        .filter_entry(|e| !should_skip(e.path()))
        .flatten()
    {
        if entry.file_type().is_file() {
            out.push(entry.path().to_path_buf());
        }
    }

    out.sort();
    Ok(out)
}

fn should_skip(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };

    matches!(name, ".git" | ".fastregex" | "target" | "node_modules")
}

fn run_ripgrep(root: &Path, pattern: &str) -> Result<usize> {
    let output = Command::new("rg")
        .arg("--pcre2")
        .arg("--json")
        .arg("--color")
        .arg("never")
        .arg("--hidden")
        .arg(pattern)
        .arg(root)
        .output()
        .context("failed to invoke rg")?;

    if !output.status.success() && output.status.code() != Some(1) {
        return Err(anyhow!(
            "rg failed for pattern '{}': {}",
            pattern,
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut matches = 0usize;

    for line in stdout.lines() {
        let value: Value = serde_json::from_str(line)?;
        if value.get("type").and_then(Value::as_str) != Some("match") {
            continue;
        }

        let submatches = value
            .get("data")
            .and_then(|d| d.get("submatches"))
            .and_then(Value::as_array)
            .map(|arr| arr.len())
            .unwrap_or(0);

        matches += submatches;
    }

    Ok(matches)
}

fn run_full_scan(files: &[PathBuf], pattern: &str) -> Result<usize> {
    let mut builder = RegexBuilder::new();
    builder.caseless(false);
    builder.dotall(false);
    builder.multi_line(false);
    let regex = builder
        .build(pattern)
        .map_err(|e| anyhow!("failed to compile pattern for full scan: {e}"))?;

    let mut matches = 0usize;
    for path in files {
        let bytes = fs::read(path)?;
        if std::str::from_utf8(&bytes).is_err() {
            continue;
        }

        for found in regex.find_iter(&bytes) {
            found?;
            matches += 1;
        }
    }

    Ok(matches)
}

fn elapsed_ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

fn percentile(mut values: Vec<f64>, p: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }

    values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));
    let rank = ((values.len() - 1) as f64 * p).round() as usize;
    values[rank.min(values.len() - 1)]
}

fn percentile_usize(mut values: Vec<usize>, p: f64) -> usize {
    if values.is_empty() {
        return 0;
    }

    values.sort_unstable();
    let rank = ((values.len() - 1) as f64 * p).round() as usize;
    values[rank.min(values.len() - 1)]
}

fn render_markdown_report(cfg: &Config, commit: &str, results: &[QueryResult]) -> String {
    let mut lines = Vec::<String>::new();

    lines.push("# FastRegex Benchmark Results".to_string());
    lines.push(String::new());
    lines.push(format!("- Dataset: `{}`", cfg.dataset_root.display()));
    lines.push(format!("- Generated files: {}", cfg.files));
    lines.push(format!("- Iterations per query: {}", cfg.iterations));
    lines.push(format!("- Base commit id: `{commit}`"));
    lines.push(String::new());

    lines.push("| Query | Pattern | Matches | Candidates (p50) | Candidate reduction | FastRegex p50/p95 (ms) | rg p50/p95 (ms) | Full scan PCRE2 p50/p95 (ms) | FastRegex vs rg (p50) |".to_string());
    lines.push("|---|---|---:|---:|---:|---:|---:|---:|---:|".to_string());

    for row in results {
        let reduction = if row.total_docs == 0 {
            0.0
        } else {
            100.0 * (1.0 - row.candidate_count as f64 / row.total_docs as f64)
        };

        let speedup_rg = if row.fast_p50_ms <= 0.0 {
            0.0
        } else {
            row.rg_p50_ms / row.fast_p50_ms
        };

        lines.push(format!(
            "| {} | `{}` | {} | {} | {:.1}% | {:.2}/{:.2} | {:.2}/{:.2} | {:.2}/{:.2} | {:.2}x |",
            row.name,
            row.pattern,
            row.match_count,
            row.candidate_count,
            reduction,
            row.fast_p50_ms,
            row.fast_p95_ms,
            row.rg_p50_ms,
            row.rg_p95_ms,
            row.full_scan_p50_ms,
            row.full_scan_p95_ms,
            speedup_rg,
        ));
    }

    lines.push(String::new());

    let mut summary = BTreeMap::new();
    for row in results {
        summary.insert(
            row.name.clone(),
            row.rg_p50_ms / row.fast_p50_ms.max(0.0001),
        );
    }

    let avg_speedup = if summary.is_empty() {
        0.0
    } else {
        summary.values().sum::<f64>() / summary.len() as f64
    };

    lines.push(format!(
        "Average speedup vs rg (p50 across queries): **{avg_speedup:.2}x**"
    ));
    lines.push("\nNotes:".to_string());
    lines.push("- Candidate reduction applies to indexed queries and may approach 0% on fallback-heavy regexes.".to_string());
    lines.push(
        "- All methods were required to return identical match counts in this benchmark run."
            .to_string(),
    );

    lines.join("\n")
}
