use std::fs::{self, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::Arc;
use std::thread;

use pcre2::bytes::RegexBuilder;
use proptest::prelude::*;
use tempfile::TempDir;

use crate::{Engine, EngineConfig, RebuildMode, SearchOptions};

fn write_file(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, content).unwrap();
}

fn build_engine(workspace: &Path) -> Engine {
    let mut cfg = EngineConfig::for_workspace(workspace);
    cfg.build.max_file_bytes = 4 * 1024 * 1024;
    Engine::new(cfg).unwrap()
}

fn brute_force_matches(workspace: &Path, pattern: &str) -> Vec<(String, usize, usize)> {
    let mut builder = RegexBuilder::new();
    builder.caseless(false);
    builder.dotall(false);
    builder.multi_line(false);
    let regex = builder.build(pattern).unwrap();

    let mut out = Vec::new();
    let entries = walkdir::WalkDir::new(workspace)
        .into_iter()
        .filter_entry(|entry| {
            let name = entry.file_name().to_string_lossy();
            !matches!(
                name.as_ref(),
                ".git" | ".fastregex" | "target" | "node_modules"
            )
        });

    for entry in entries.flatten() {
        if !entry.file_type().is_file() {
            continue;
        }

        let path = entry.path();
        let rel = path
            .strip_prefix(workspace)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");

        let bytes = fs::read(path).unwrap();
        for found in regex.find_iter(&bytes).flatten() {
            out.push((rel.clone(), found.start(), found.end()));
        }
    }

    out.sort();
    out
}

#[test]
fn literal_search_matches_bruteforce() {
    let tmp = TempDir::new().unwrap();
    write_file(&tmp.path().join("src/a.txt"), "hello world\nhello rust\n");
    write_file(&tmp.path().join("src/b.txt"), "another line\n");

    let engine = build_engine(tmp.path());
    let response = engine
        .regex_search("hello", SearchOptions::default())
        .unwrap();

    let mut got: Vec<(String, usize, usize)> = response
        .matches
        .into_iter()
        .map(|m| (m.path, m.byte_offset, m.end_offset))
        .collect();
    got.sort();

    let expected = brute_force_matches(tmp.path(), "hello");
    assert_eq!(got, expected);
}

#[test]
fn read_your_own_writes_overlay() {
    let tmp = TempDir::new().unwrap();
    let file = tmp.path().join("src/main.txt");
    write_file(&file, "before\n");

    let engine = build_engine(tmp.path());
    let before = engine
        .regex_search("after", SearchOptions::default())
        .unwrap();
    assert!(before.matches.is_empty());

    write_file(&file, "after value\n");
    let update = engine
        .index_update_files(&["src/main.txt".to_string()])
        .unwrap();
    assert_eq!(update.updated, 1);

    let after = engine
        .regex_search("after", SearchOptions::default())
        .unwrap();
    assert_eq!(after.matches.len(), 1);
    assert_eq!(after.matches[0].path, "src/main.txt");
}

#[test]
fn concurrent_search_and_overlay_updates() {
    let tmp = TempDir::new().unwrap();
    let file = tmp.path().join("src/live.txt");
    write_file(&file, "alpha\nalpha\nalpha\n");

    let engine = Arc::new(build_engine(tmp.path()));

    let search_engine = Arc::clone(&engine);
    let searcher = thread::spawn(move || {
        for _ in 0..50 {
            let response = search_engine
                .regex_search("alpha", SearchOptions::default())
                .unwrap();
            assert!(!response.matches.is_empty());
        }
    });

    let update_engine = Arc::clone(&engine);
    let updater = thread::spawn(move || {
        for idx in 0..20 {
            let content = format!("alpha {idx}\n");
            fs::write(&file, content).unwrap();
            let _ = update_engine.index_update_files(&["src/live.txt".to_string()]);
        }
    });

    searcher.join().unwrap();
    updater.join().unwrap();
}

#[test]
fn corrupt_index_triggers_rebuild_on_restart() {
    let tmp = TempDir::new().unwrap();
    write_file(&tmp.path().join("src/x.txt"), "needle\n");

    let mut cfg = EngineConfig::for_workspace(tmp.path());
    cfg.build.max_file_bytes = 4 * 1024 * 1024;

    let engine = Engine::new(cfg.clone()).unwrap();
    let status = engine.index_status().unwrap();

    let postings_path = cfg
        .index_root
        .join(status.repo_id)
        .join(status.base_commit)
        .join("postings.bin");

    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&postings_path)
        .unwrap();

    // Corrupt checksum field in postings header.
    file.seek(SeekFrom::Start(12)).unwrap();
    file.write_all(&0u64.to_le_bytes()).unwrap();
    file.flush().unwrap();

    drop(engine);

    let rebuilt = Engine::new(cfg).unwrap();
    let response = rebuilt
        .regex_search("needle", SearchOptions::default())
        .unwrap();
    assert_eq!(response.matches.len(), 1);
}

proptest! {
    #[test]
    fn property_literal_queries_match_full_scan(pattern in "[a-z]{3,8}") {
        let tmp = TempDir::new().unwrap();

        write_file(&tmp.path().join("a.txt"), "alpha beta gamma\ndelta epsilon\n");
        write_file(&tmp.path().join("b.txt"), "rust regex search\nfast engine\n");
        write_file(&tmp.path().join("c.txt"), "cursor fastregex mcp\n");

        let engine = build_engine(tmp.path());
        let response = engine.regex_search(&pattern, SearchOptions::default()).unwrap();

        let mut got: Vec<(String, usize, usize)> = response.matches
            .into_iter()
            .map(|m| (m.path, m.byte_offset, m.end_offset))
            .collect();
        got.sort();

        let expected = brute_force_matches(tmp.path(), &pattern);
        prop_assert_eq!(got, expected);
    }
}

#[test]
fn explicit_rebuild_foreground_works() {
    let tmp = TempDir::new().unwrap();
    write_file(&tmp.path().join("src/y.txt"), "needle\n");

    let engine = build_engine(tmp.path());
    let rebuild = engine.index_rebuild(RebuildMode::Foreground).unwrap();
    assert!(rebuild.doc_count >= 1);
}
