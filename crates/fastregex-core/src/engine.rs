use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use parking_lot::RwLock;
use pcre2::bytes::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};

use crate::error::{FastRegexError, Result};
use crate::filters::PathFilter;
use crate::hashing::hash_repo_id;
use crate::index::{
    BuildConfig, BuildStats, IndexSnapshot, build_and_write_index, discover_repo_files,
    extract_index_hashes,
};
use crate::overlay::{OverlayEntry, OverlayStore};
use crate::planner::{PlanExpr, build_query_plan};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchOptions {
    #[serde(default)]
    pub include: Vec<String>,
    #[serde(default)]
    pub exclude: Vec<String>,
    #[serde(default)]
    pub globs: Vec<String>,
    #[serde(default = "default_max_results")]
    pub max_results: usize,
    #[serde(default = "default_case_sensitive")]
    pub case_sensitive: bool,
    #[serde(default)]
    pub dotall: bool,
    #[serde(default)]
    pub multiline: bool,
    #[serde(default)]
    pub no_snippet: bool,
    pub timeout_ms: Option<u64>,
    pub request_id: Option<String>,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            include: Vec::new(),
            exclude: Vec::new(),
            globs: Vec::new(),
            max_results: default_max_results(),
            case_sensitive: default_case_sensitive(),
            dotall: false,
            multiline: false,
            no_snippet: false,
            timeout_ms: None,
            request_id: None,
        }
    }
}

fn default_max_results() -> usize {
    200
}

fn default_case_sensitive() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchMatch {
    pub path: String,
    pub byte_offset: usize,
    pub end_offset: usize,
    pub line: usize,
    pub column: usize,
    pub snippet: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResponse {
    pub matches: Vec<SearchMatch>,
    pub candidate_count: usize,
    pub used_fallback: bool,
    pub extracted_literals: Vec<String>,
    pub base_generation: u64,
    pub overlay_generation: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RebuildMode {
    Foreground,
    Background,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum RebuildState {
    Idle,
    Running,
    Failed { message: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexStatus {
    pub base_commit: String,
    pub current_commit: String,
    pub repo_id: String,
    pub freshness: String,
    pub overlay_dirty_files: usize,
    pub rebuild_state: RebuildState,
    pub indexed_docs: usize,
    pub base_generation: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OverlayUpdateResult {
    pub updated: usize,
    pub deleted: usize,
    pub skipped: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexRebuildResult {
    pub mode: RebuildMode,
    pub base_commit: String,
    pub doc_count: usize,
    pub posting_count: usize,
    pub rebuild_state: RebuildState,
}

#[derive(Debug, Clone)]
pub struct EngineConfig {
    pub workspace_root: PathBuf,
    pub index_root: PathBuf,
    pub build: BuildConfig,
}

impl EngineConfig {
    pub fn for_workspace(workspace_root: impl AsRef<Path>) -> Self {
        let workspace_root = workspace_root.as_ref().to_path_buf();
        Self {
            index_root: workspace_root.join(".fastregex").join("index"),
            workspace_root,
            build: BuildConfig::default(),
        }
    }
}

#[derive(Debug)]
struct BaseIndexState {
    snapshot: Arc<IndexSnapshot>,
    generation: u64,
    indexed_at: SystemTime,
}

#[derive(Debug)]
struct EngineInner {
    config: EngineConfig,
    repo_id: String,
    base: RwLock<BaseIndexState>,
    overlay: OverlayStore,
    rebuild_state: RwLock<RebuildState>,
}

#[derive(Clone, Debug)]
pub struct Engine {
    inner: Arc<EngineInner>,
}

impl Engine {
    pub fn new(config: EngineConfig) -> Result<Self> {
        fs::create_dir_all(&config.index_root)?;

        let workspace_abs = config
            .workspace_root
            .canonicalize()
            .unwrap_or_else(|_| config.workspace_root.clone());
        let repo_id = hash_repo_id(&workspace_abs.to_string_lossy());

        let commit = detect_head_commit(&workspace_abs);
        let (snapshot, _) = load_or_build_index(&config, &repo_id, &commit)?;

        let base = BaseIndexState {
            snapshot: Arc::new(snapshot),
            generation: 1,
            indexed_at: SystemTime::now(),
        };

        Ok(Self {
            inner: Arc::new(EngineInner {
                config: EngineConfig {
                    workspace_root: workspace_abs,
                    index_root: config.index_root,
                    build: config.build,
                },
                repo_id,
                base: RwLock::new(base),
                overlay: OverlayStore::default(),
                rebuild_state: RwLock::new(RebuildState::Idle),
            }),
        })
    }

    pub fn regex_search(&self, pattern: &str, options: SearchOptions) -> Result<SearchResponse> {
        if pattern.is_empty() {
            return Err(FastRegexError::InvalidRequest(
                "pattern cannot be empty".to_string(),
            ));
        }

        let max_results = if options.max_results == 0 {
            default_max_results()
        } else {
            options.max_results
        };

        let mut regex_builder = RegexBuilder::new();
        regex_builder.caseless(!options.case_sensitive);
        regex_builder.dotall(options.dotall);
        regex_builder.multi_line(options.multiline);
        let regex = regex_builder
            .build(pattern)
            .map_err(|err| FastRegexError::RegexCompile(err.to_string()))?;

        let deadline = options
            .timeout_ms
            .map(|ms| (Instant::now() + Duration::from_millis(ms), ms));

        let (index, base_generation) = {
            let base = self.inner.base.read();
            (Arc::clone(&base.snapshot), base.generation)
        };

        let filter = PathFilter::new(&options.include, &options.globs, &options.exclude)?;
        let plan = build_query_plan(
            pattern,
            &index.bigram_frequency,
            self.inner.config.build.sparse_config(),
        );

        let mut base_candidates = self.base_candidates(&index, &plan.expr, &filter)?;
        let overlay_snapshot = self.inner.overlay.snapshot();
        let overlay_generation = overlay_snapshot.generation;

        let mut overlay_candidates = Vec::<String>::new();

        for (path, entry) in &overlay_snapshot.files {
            if let Some(doc_id) = index.doc_id_for_path(path) {
                base_candidates.remove(&doc_id);
            }

            if !filter.allows(path) {
                continue;
            }

            match entry {
                OverlayEntry::Deleted => {}
                OverlayEntry::Modified(file) => {
                    if plan.matches_hashes(&file.gram_hashes) {
                        overlay_candidates.push(path.clone());
                    }
                }
            }
        }

        let mut base_ids: Vec<u32> = base_candidates.into_iter().collect();
        base_ids.sort_unstable();
        overlay_candidates.sort();
        overlay_candidates.dedup();

        let candidate_count = base_ids.len() + overlay_candidates.len();

        let mut out = Vec::<SearchMatch>::new();

        for doc_id in base_ids {
            check_deadline(deadline, &options.request_id)?;

            let Some(doc) = index.doc_by_id(doc_id) else {
                continue;
            };

            let Some(bytes) = read_bytes(&self.inner.config.workspace_root.join(&doc.path))? else {
                continue;
            };

            scan_candidate(
                &regex,
                &doc.path,
                &bytes,
                &mut out,
                max_results,
                deadline,
                &options.request_id,
                !options.no_snippet,
            )?;

            if out.len() >= max_results {
                break;
            }
        }

        if out.len() < max_results {
            for path in overlay_candidates {
                check_deadline(deadline, &options.request_id)?;

                let Some(entry) = overlay_snapshot.files.get(&path) else {
                    continue;
                };

                if let OverlayEntry::Modified(file) = entry {
                    scan_candidate(
                        &regex,
                        &path,
                        file.text.as_bytes(),
                        &mut out,
                        max_results,
                        deadline,
                        &options.request_id,
                        !options.no_snippet,
                    )?;
                }

                if out.len() >= max_results {
                    break;
                }
            }
        }

        Ok(SearchResponse {
            matches: out,
            candidate_count,
            used_fallback: plan.used_fallback,
            extracted_literals: plan.extracted_literals,
            base_generation,
            overlay_generation,
        })
    }

    pub fn index_status(&self) -> Result<IndexStatus> {
        let current_commit = detect_head_commit(&self.inner.config.workspace_root);
        let base = self.inner.base.read();
        let overlay_dirty = self.inner.overlay.dirty_files();

        let freshness = if base.snapshot.commit_id == current_commit && overlay_dirty == 0 {
            "fresh"
        } else {
            "stale"
        }
        .to_string();

        Ok(IndexStatus {
            base_commit: base.snapshot.commit_id.clone(),
            current_commit,
            repo_id: base.snapshot.repo_id.clone(),
            freshness,
            overlay_dirty_files: overlay_dirty,
            rebuild_state: self.inner.rebuild_state.read().clone(),
            indexed_docs: base.snapshot.docs.len(),
            base_generation: base.generation,
        })
    }

    pub fn index_update_files(&self, changed_files: &[String]) -> Result<OverlayUpdateResult> {
        let base = self.inner.base.read();
        let bigram_frequency = base.snapshot.bigram_frequency.clone();
        drop(base);

        let mut updated = 0usize;
        let mut deleted = 0usize;
        let mut skipped = 0usize;

        for input in changed_files {
            let rel_path = normalize_input_path(&self.inner.config.workspace_root, input);
            let abs_path = self.inner.config.workspace_root.join(&rel_path);

            if !abs_path.exists() {
                self.inner.overlay.upsert_deleted(rel_path);
                deleted += 1;
                continue;
            }

            if !abs_path.is_file() {
                skipped += 1;
                continue;
            }

            let Some(bytes) = read_utf8_bytes(&abs_path)? else {
                skipped += 1;
                continue;
            };

            let text = String::from_utf8(bytes).map_err(|err| FastRegexError::Utf8 {
                path: abs_path.clone(),
                message: err.to_string(),
            })?;

            let gram_hashes =
                extract_index_hashes(text.as_bytes(), &bigram_frequency, &self.inner.config.build);

            self.inner
                .overlay
                .upsert_modified(rel_path, text, gram_hashes);
            updated += 1;
        }

        Ok(OverlayUpdateResult {
            updated,
            deleted,
            skipped,
        })
    }

    pub fn index_rebuild(&self, mode: RebuildMode) -> Result<IndexRebuildResult> {
        match mode {
            RebuildMode::Foreground => self.rebuild_foreground(),
            RebuildMode::Background => self.rebuild_background(),
        }
    }

    fn rebuild_foreground(&self) -> Result<IndexRebuildResult> {
        {
            let mut state = self.inner.rebuild_state.write();
            *state = RebuildState::Running;
        }

        let outcome = self.perform_rebuild();

        match outcome {
            Ok((stats, commit)) => {
                *self.inner.rebuild_state.write() = RebuildState::Idle;
                Ok(IndexRebuildResult {
                    mode: RebuildMode::Foreground,
                    base_commit: commit,
                    doc_count: stats.doc_count,
                    posting_count: stats.posting_count,
                    rebuild_state: RebuildState::Idle,
                })
            }
            Err(err) => {
                *self.inner.rebuild_state.write() = RebuildState::Failed {
                    message: err.to_string(),
                };
                Err(err)
            }
        }
    }

    fn rebuild_background(&self) -> Result<IndexRebuildResult> {
        {
            let mut state = self.inner.rebuild_state.write();
            if matches!(*state, RebuildState::Running) {
                return Err(FastRegexError::RebuildAlreadyRunning);
            }
            *state = RebuildState::Running;
        }

        let engine = self.clone();
        std::thread::spawn(move || {
            let outcome = engine.perform_rebuild();
            match outcome {
                Ok(_) => {
                    *engine.inner.rebuild_state.write() = RebuildState::Idle;
                }
                Err(err) => {
                    *engine.inner.rebuild_state.write() = RebuildState::Failed {
                        message: err.to_string(),
                    };
                }
            }
        });

        let status = self.index_status()?;
        Ok(IndexRebuildResult {
            mode: RebuildMode::Background,
            base_commit: status.base_commit,
            doc_count: status.indexed_docs,
            posting_count: 0,
            rebuild_state: RebuildState::Running,
        })
    }

    fn perform_rebuild(&self) -> Result<(BuildStats, String)> {
        let commit = detect_head_commit(&self.inner.config.workspace_root);
        let files = discover_repo_files(&self.inner.config.workspace_root, &commit);
        let commit_dir = self
            .inner
            .config
            .index_root
            .join(&self.inner.repo_id)
            .join(&commit);

        let stats = build_and_write_index(
            &self.inner.config.workspace_root,
            &files,
            &commit_dir,
            &self.inner.repo_id,
            &commit,
            &self.inner.config.build,
        )?;

        let snapshot = IndexSnapshot::load_from(&commit_dir)?;

        {
            let mut base = self.inner.base.write();
            base.snapshot = Arc::new(snapshot);
            base.generation = base.generation.saturating_add(1);
            base.indexed_at = SystemTime::now();
        }

        Ok((stats, commit))
    }

    fn base_candidates(
        &self,
        index: &IndexSnapshot,
        expr: &PlanExpr,
        filter: &PathFilter,
    ) -> Result<HashSet<u32>> {
        match expr {
            PlanExpr::AllDocs => Ok(index
                .all_doc_ids()
                .filter(|doc_id| {
                    index
                        .doc_by_id(*doc_id)
                        .map(|doc| filter.allows(&doc.path))
                        .unwrap_or(false)
                })
                .collect()),
            PlanExpr::And(hashes) => {
                let mut filtered: HashSet<u32> = Self::intersect_hashes(index, hashes)?
                    .into_iter()
                    .filter(|doc_id| {
                        index
                            .doc_by_id(*doc_id)
                            .map(|doc| filter.allows(&doc.path))
                            .unwrap_or(false)
                    })
                    .collect();

                filtered.extend(index.unindexed_doc_ids().filter(|doc_id| {
                    index
                        .doc_by_id(*doc_id)
                        .map(|doc| filter.allows(&doc.path))
                        .unwrap_or(false)
                }));

                Ok(filtered)
            }
            PlanExpr::Or(branches) => {
                let mut out = HashSet::<u32>::new();

                for branch in branches {
                    out.extend(Self::intersect_hashes(index, branch)?);
                }

                out.retain(|doc_id| {
                    index
                        .doc_by_id(*doc_id)
                        .map(|doc| filter.allows(&doc.path))
                        .unwrap_or(false)
                });

                out.extend(index.unindexed_doc_ids().filter(|doc_id| {
                    index
                        .doc_by_id(*doc_id)
                        .map(|doc| filter.allows(&doc.path))
                        .unwrap_or(false)
                }));

                Ok(out)
            }
        }
    }

    fn intersect_hashes(index: &IndexSnapshot, hashes: &[u64]) -> Result<HashSet<u32>> {
        if hashes.is_empty() {
            return Ok(HashSet::new());
        }

        let mut posting_lists = Vec::<Vec<u32>>::with_capacity(hashes.len());
        for hash in hashes {
            let posting = index.posting_for_hash(*hash)?.unwrap_or_default();
            if posting.is_empty() {
                return Ok(HashSet::new());
            }
            posting_lists.push(posting);
        }

        posting_lists.sort_by_key(|list| list.len());
        let mut acc = posting_lists[0].clone();
        for posting in posting_lists.iter().skip(1) {
            acc = intersect_sorted_doc_ids(&acc, posting);
            if acc.is_empty() {
                return Ok(HashSet::new());
            }
        }

        Ok(acc.into_iter().collect())
    }
}

fn load_or_build_index(
    config: &EngineConfig,
    repo_id: &str,
    commit: &str,
) -> Result<(IndexSnapshot, BuildStats)> {
    let commit_dir = config.index_root.join(repo_id).join(commit);

    if commit_dir.join("postings.bin").exists() && commit_dir.join("lookup.bin").exists() {
        match IndexSnapshot::load_from(&commit_dir) {
            Ok(snapshot) => {
                let stats = BuildStats {
                    doc_count: snapshot.docs.len(),
                    posting_count: 0,
                };
                return Ok((snapshot, stats));
            }
            Err(FastRegexError::CorruptIndex(_)) => {}
            Err(err) => return Err(err),
        }
    }

    let files = discover_repo_files(&config.workspace_root, commit);
    let stats = build_and_write_index(
        &config.workspace_root,
        &files,
        &commit_dir,
        repo_id,
        commit,
        &config.build,
    )?;

    let snapshot = IndexSnapshot::load_from(&commit_dir)?;
    Ok((snapshot, stats))
}

fn detect_head_commit(workspace: &Path) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(workspace)
        .arg("rev-parse")
        .arg("HEAD")
        .output();

    let Ok(output) = output else {
        return "NO_GIT".to_string();
    };

    if !output.status.success() {
        return "NO_GIT".to_string();
    }

    let commit = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if commit.is_empty() {
        "NO_GIT".to_string()
    } else {
        commit
    }
}

fn normalize_input_path(workspace: &Path, input: &str) -> String {
    let path = Path::new(input);

    if path.is_absolute() {
        if let Ok(rel) = path.strip_prefix(workspace) {
            return rel.to_string_lossy().replace('\\', "/");
        }
        return path.to_string_lossy().replace('\\', "/");
    }

    path.to_string_lossy().replace('\\', "/")
}

fn read_bytes(path: &Path) -> Result<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err.into()),
    }
}

fn read_utf8_bytes(path: &Path) -> Result<Option<Vec<u8>>> {
    let bytes = match read_bytes(path)? {
        Some(bytes) => bytes,
        None => return Ok(None),
    };

    if std::str::from_utf8(&bytes).is_err() {
        return Ok(None);
    }

    Ok(Some(bytes))
}

fn check_deadline(deadline: Option<(Instant, u64)>, request_id: &Option<String>) -> Result<()> {
    if let Some((when, timeout_ms)) = deadline {
        if Instant::now() > when {
            return Err(FastRegexError::Timeout {
                request_id: request_id.clone(),
                timeout_ms,
            });
        }
    }

    Ok(())
}

fn scan_candidate(
    regex: &Regex,
    path: &str,
    bytes: &[u8],
    out: &mut Vec<SearchMatch>,
    max_results: usize,
    deadline: Option<(Instant, u64)>,
    request_id: &Option<String>,
    include_snippet: bool,
) -> Result<()> {
    let line_starts = build_line_starts(bytes);

    for result in regex.find_iter(bytes) {
        check_deadline(deadline, request_id)?;

        let found = result.map_err(|err| FastRegexError::Internal(err.to_string()))?;
        let start = found.start();
        let end = found.end();

        let (line, column, snippet) = if include_snippet {
            line_column_snippet(bytes, &line_starts, start)
        } else {
            let (line, column) = line_column_only(&line_starts, start);
            (line, column, String::new())
        };

        out.push(SearchMatch {
            path: path.to_string(),
            byte_offset: start,
            end_offset: end,
            line,
            column,
            snippet,
        });

        if out.len() >= max_results {
            break;
        }
    }

    Ok(())
}

fn build_line_starts(bytes: &[u8]) -> Vec<usize> {
    let mut starts = Vec::<usize>::new();
    starts.push(0);

    for (idx, b) in bytes.iter().enumerate() {
        if *b == b'\n' && idx + 1 < bytes.len() {
            starts.push(idx + 1);
        }
    }

    starts
}

fn intersect_sorted_doc_ids(left: &[u32], right: &[u32]) -> Vec<u32> {
    let mut out = Vec::<u32>::with_capacity(left.len().min(right.len()));
    let mut i = 0usize;
    let mut j = 0usize;

    while i < left.len() && j < right.len() {
        match left[i].cmp(&right[j]) {
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
            std::cmp::Ordering::Equal => {
                out.push(left[i]);
                i += 1;
                j += 1;
            }
        }
    }

    out
}

fn line_column_only(line_starts: &[usize], start: usize) -> (usize, usize) {
    let line_idx = line_starts.partition_point(|&line_start| line_start <= start);
    let line_idx = line_idx.saturating_sub(1);
    let line_start = line_starts.get(line_idx).copied().unwrap_or(0);
    let column = start.saturating_sub(line_start).saturating_add(1);
    (line_idx + 1, column)
}

fn line_column_snippet(bytes: &[u8], line_starts: &[usize], start: usize) -> (usize, usize, String) {
    let bounded_start = start.min(bytes.len());
    let line_idx = line_starts.partition_point(|&line_start| line_start <= bounded_start);
    let line_idx = line_idx.saturating_sub(1);
    let line_start = line_starts.get(line_idx).copied().unwrap_or(0);

    let line_end = if line_idx + 1 < line_starts.len() {
        line_starts[line_idx + 1].saturating_sub(1)
    } else {
        bytes.len()
    };

    let column = bounded_start.saturating_sub(line_start).saturating_add(1);
    let snippet = if line_start <= line_end && line_end <= bytes.len() {
        String::from_utf8_lossy(&bytes[line_start..line_end]).to_string()
    } else {
        String::new()
    };

    (line_idx + 1, column, snippet)
}
