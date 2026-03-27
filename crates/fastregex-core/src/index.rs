use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::UNIX_EPOCH;

use memmap2::MmapOptions;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use walkdir::WalkDir;

use crate::error::{FastRegexError, Result};
use crate::hashing::hash_gram;
use crate::sparse::{SparseConfig, build_all_sparse_ngrams, build_bigram_frequency};

const POSTINGS_MAGIC: &[u8; 8] = b"FRPOSTV1";
const LOOKUP_MAGIC: &[u8; 8] = b"FRLOOKV1";
const POSTINGS_VERSION: u32 = 1;
const LOOKUP_VERSION: u32 = 1;
const POSTINGS_HEADER_SIZE: usize = 256;
const LOOKUP_HEADER_SIZE: usize = 64;
const LOOKUP_ENTRY_SIZE: usize = 16;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildConfig {
    pub max_span_bigrams: usize,
    pub max_grams_per_file: usize,
    pub max_covering_grams: usize,
    pub max_file_bytes: usize,
}

impl Default for BuildConfig {
    fn default() -> Self {
        Self {
            max_span_bigrams: 24,
            max_grams_per_file: 40_000,
            max_covering_grams: 12,
            max_file_bytes: 2 * 1024 * 1024,
        }
    }
}

impl BuildConfig {
    pub fn sparse_config(&self) -> SparseConfig {
        SparseConfig {
            max_span_bigrams: self.max_span_bigrams,
            max_grams_per_text: self.max_grams_per_file,
            max_covering_grams: self.max_covering_grams,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildStats {
    pub doc_count: usize,
    pub posting_count: usize,
}

#[derive(Debug, Clone)]
pub struct DocumentMeta {
    pub doc_id: u32,
    pub path: String,
    pub size: u64,
    pub mtime_secs: i64,
    pub indexed: bool,
}

#[derive(Debug)]
pub struct IndexSnapshot {
    pub repo_id: String,
    pub commit_id: String,
    pub docs: Vec<DocumentMeta>,
    pub bigram_frequency: HashMap<u16, u32>,
    lookup_mmap: memmap2::Mmap,
    postings_mmap: memmap2::Mmap,
    lookup_entry_count: usize,
    path_to_doc: HashMap<String, u32>,
    posting_cache: Mutex<PostingCache>,
}

impl IndexSnapshot {
    pub fn load_from(index_dir: &Path) -> Result<Self> {
        let postings_path = index_dir.join("postings.bin");
        let lookup_path = index_dir.join("lookup.bin");

        let postings_file = File::open(&postings_path)?;
        let lookup_file = File::open(&lookup_path)?;

        let postings_mmap = unsafe { MmapOptions::new().map(&postings_file)? };
        let lookup_mmap = unsafe { MmapOptions::new().map(&lookup_file)? };

        let postings_header = decode_postings_header(&postings_mmap)?;
        validate_payload_checksum(
            &postings_mmap,
            POSTINGS_HEADER_SIZE,
            postings_header.checksum,
            "postings.bin",
        )?;

        let lookup_header = decode_lookup_header(&lookup_mmap)?;
        validate_payload_checksum(
            &lookup_mmap,
            LOOKUP_HEADER_SIZE,
            lookup_header.checksum,
            "lookup.bin",
        )?;

        if postings_header.posting_count as usize != lookup_header.entry_count as usize {
            return Err(FastRegexError::CorruptIndex(
                "posting_count mismatch between postings.bin and lookup.bin".to_string(),
            ));
        }

        let docs = decode_doc_table(&postings_mmap, &postings_header)?;
        let bigram_frequency = decode_bigram_table(&postings_mmap, &postings_header)?;

        let mut path_to_doc = HashMap::with_capacity(docs.len());
        for doc in &docs {
            path_to_doc.insert(doc.path.clone(), doc.doc_id);
        }

        Ok(Self {
            repo_id: postings_header.repo_id,
            commit_id: postings_header.commit_id,
            docs,
            bigram_frequency,
            lookup_mmap,
            postings_mmap,
            lookup_entry_count: lookup_header.entry_count as usize,
            path_to_doc,
            posting_cache: Mutex::new(PostingCache::new(4096)),
        })
    }

    pub fn doc_id_for_path(&self, path: &str) -> Option<u32> {
        self.path_to_doc.get(path).copied()
    }

    pub fn doc_by_id(&self, doc_id: u32) -> Option<&DocumentMeta> {
        self.docs.get(doc_id as usize)
    }

    pub fn all_doc_ids(&self) -> impl Iterator<Item = u32> + '_ {
        self.docs.iter().map(|d| d.doc_id)
    }

    pub fn unindexed_doc_ids(&self) -> impl Iterator<Item = u32> + '_ {
        self.docs.iter().filter(|d| !d.indexed).map(|d| d.doc_id)
    }

    pub fn posting_for_hash(&self, hash: u64) -> Result<Option<Vec<u32>>> {
        if let Some(cached) = self.posting_cache.lock().get(hash) {
            return Ok(Some(cached));
        }

        let mut low = 0usize;
        let mut high = self.lookup_entry_count;

        while low < high {
            let mid = (low + high) / 2;
            let (mid_hash, offset) = self.lookup_entry(mid)?;

            if mid_hash == hash {
                let docs = self.decode_posting(offset as usize)?;
                self.posting_cache.lock().insert(hash, docs.clone());
                return Ok(Some(docs));
            }

            if mid_hash < hash {
                low = mid + 1;
            } else {
                high = mid;
            }
        }

        Ok(None)
    }

    fn lookup_entry(&self, idx: usize) -> Result<(u64, u64)> {
        let start = LOOKUP_HEADER_SIZE + idx * LOOKUP_ENTRY_SIZE;
        let end = start + LOOKUP_ENTRY_SIZE;
        if end > self.lookup_mmap.len() {
            return Err(FastRegexError::CorruptIndex(
                "lookup entry out of bounds".to_string(),
            ));
        }

        let hash = read_u64(&self.lookup_mmap, start)?;
        let offset = read_u64(&self.lookup_mmap, start + 8)?;
        Ok((hash, offset))
    }

    fn decode_posting(&self, offset: usize) -> Result<Vec<u32>> {
        if offset + 4 > self.postings_mmap.len() {
            return Err(FastRegexError::CorruptIndex(
                "posting offset out of bounds".to_string(),
            ));
        }

        let count = read_u32(&self.postings_mmap, offset)? as usize;
        let mut cursor = offset + 4;
        let mut out = Vec::with_capacity(count);

        for _ in 0..count {
            let doc_id = read_u32(&self.postings_mmap, cursor)?;
            out.push(doc_id);
            cursor += 4;
        }

        Ok(out)
    }
}

pub fn discover_repo_files(workspace: &Path, commit_id: &str) -> Vec<PathBuf> {
    if let Ok(files) = git_list_files(workspace, commit_id) {
        if !files.is_empty() {
            return files;
        }
    }

    let mut out = Vec::new();
    let walker = WalkDir::new(workspace)
        .into_iter()
        .filter_entry(|entry| !should_skip(entry.path()));

    for entry in walker.flatten() {
        if entry.file_type().is_file() {
            out.push(entry.into_path());
        }
    }

    out
}

pub fn build_and_write_index(
    workspace: &Path,
    files: &[PathBuf],
    index_dir: &Path,
    repo_id: &str,
    commit_id: &str,
    config: &BuildConfig,
) -> Result<BuildStats> {
    fs::create_dir_all(index_dir)?;

    let mut docs_raw = Vec::<(String, u64, i64, bool)>::new();
    let mut bigram_frequency = HashMap::<u16, u32>::new();

    for abs in files {
        if !abs.is_file() {
            continue;
        }

        let rel = to_relative_path(workspace, abs);
        if rel.starts_with(".git/") || rel.starts_with(".fastregex/") {
            continue;
        }

        let metadata = fs::metadata(abs)?;
        let mtime_secs = metadata
            .modified()
            .ok()
            .and_then(|mtime| mtime.duration_since(UNIX_EPOCH).ok())
            .map(|dur| dur.as_secs() as i64)
            .unwrap_or(0);

        let indexed = match read_utf8_state(abs, config.max_file_bytes)? {
            Utf8Read::Indexed(bytes) => {
                build_bigram_frequency(&bytes, &mut bigram_frequency);
                true
            }
            Utf8Read::UnindexedUtf8 => false,
            Utf8Read::NonUtf8 | Utf8Read::Missing => continue,
        };

        docs_raw.push((rel, metadata.len(), mtime_secs, indexed));
    }

    docs_raw.sort_by(|a, b| a.0.cmp(&b.0));

    let docs: Vec<DocumentMeta> = docs_raw
        .into_iter()
        .enumerate()
        .map(|(idx, (path, size, mtime_secs, indexed))| DocumentMeta {
            doc_id: idx as u32,
            path,
            size,
            mtime_secs,
            indexed,
        })
        .collect();

    let mut buckets = HashMap::<u64, Vec<u32>>::new();

    for doc in &docs {
        if !doc.indexed {
            continue;
        }

        let abs = workspace.join(&doc.path);
        let Utf8Read::Indexed(bytes) = read_utf8_state(&abs, config.max_file_bytes)? else {
            continue;
        };

        let hashes = extract_index_hashes(&bytes, &bigram_frequency, config);
        for hash in hashes {
            buckets.entry(hash).or_default().push(doc.doc_id);
        }
    }

    let mut hashes: Vec<u64> = buckets.keys().copied().collect();
    hashes.sort_unstable();

    for docs in buckets.values_mut() {
        docs.sort_unstable();
        docs.dedup();
    }

    let postings_path = index_dir.join("postings.bin");
    let lookup_path = index_dir.join("lookup.bin");

    let lookup_entries = write_postings(
        &postings_path,
        repo_id,
        commit_id,
        &docs,
        &hashes,
        &buckets,
        &bigram_frequency,
    )?;
    write_lookup(&lookup_path, &lookup_entries)?;

    Ok(BuildStats {
        doc_count: docs.len(),
        posting_count: lookup_entries.len(),
    })
}

pub fn extract_index_hashes(
    text: &[u8],
    bigram_frequency: &HashMap<u16, u32>,
    config: &BuildConfig,
) -> HashSet<u64> {
    let mut out = HashSet::<u64>::new();

    if text.len() >= 3 {
        for tri in text.windows(3) {
            out.insert(hash_gram(tri));
        }
    }

    let sparse_cfg = config.sparse_config();
    for gram in build_all_sparse_ngrams(text, bigram_frequency, sparse_cfg) {
        out.insert(hash_gram(&gram));
        if out.len() >= config.max_grams_per_file {
            break;
        }
    }

    out
}

#[derive(Debug, Clone)]
struct PostingsHeader {
    checksum: u64,
    doc_table_offset: u64,
    postings_offset: u64,
    bigram_offset: u64,
    doc_count: u32,
    posting_count: u32,
    repo_id: String,
    commit_id: String,
}

#[derive(Debug, Clone)]
struct LookupHeader {
    checksum: u64,
    entry_count: u32,
}

#[derive(Debug)]
struct PostingCache {
    cap: usize,
    order: VecDeque<u64>,
    map: HashMap<u64, Vec<u32>>,
}

impl PostingCache {
    fn new(cap: usize) -> Self {
        Self {
            cap,
            order: VecDeque::new(),
            map: HashMap::new(),
        }
    }

    fn get(&mut self, hash: u64) -> Option<Vec<u32>> {
        let value = self.map.get(&hash).cloned()?;
        self.bump(hash);
        Some(value)
    }

    fn insert(&mut self, hash: u64, docs: Vec<u32>) {
        if self.map.contains_key(&hash) {
            self.map.insert(hash, docs);
            self.bump(hash);
            return;
        }

        if self.map.len() >= self.cap {
            if let Some(old) = self.order.pop_front() {
                self.map.remove(&old);
            }
        }

        self.map.insert(hash, docs);
        self.order.push_back(hash);
    }

    fn bump(&mut self, hash: u64) {
        if let Some(pos) = self.order.iter().position(|h| *h == hash) {
            self.order.remove(pos);
            self.order.push_back(hash);
        }
    }
}

fn should_skip(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };

    matches!(name, ".git" | ".fastregex" | "target" | "node_modules")
}

fn to_relative_path(workspace: &Path, path: &Path) -> String {
    match path.strip_prefix(workspace) {
        Ok(rel) => rel.to_string_lossy().replace('\\', "/"),
        Err(_) => path.to_string_lossy().replace('\\', "/"),
    }
}

enum Utf8Read {
    Indexed(Vec<u8>),
    UnindexedUtf8,
    NonUtf8,
    Missing,
}

fn read_utf8_state(path: &Path, max_bytes: usize) -> Result<Utf8Read> {
    let metadata = match fs::metadata(path) {
        Ok(meta) => meta,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Utf8Read::Missing),
        Err(err) => return Err(err.into()),
    };

    let mut file = File::open(path)?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.read_to_end(&mut bytes)?;

    if std::str::from_utf8(&bytes).is_err() {
        return Ok(Utf8Read::NonUtf8);
    }

    if bytes.len() > max_bytes {
        return Ok(Utf8Read::UnindexedUtf8);
    }

    Ok(Utf8Read::Indexed(bytes))
}

fn git_list_files(workspace: &Path, commit_id: &str) -> Result<Vec<PathBuf>> {
    let commit = if commit_id.is_empty() {
        "HEAD"
    } else {
        commit_id
    };

    let output = Command::new("git")
        .arg("-C")
        .arg(workspace)
        .arg("ls-tree")
        .arg("-r")
        .arg("--name-only")
        .arg(commit)
        .output();

    let Ok(output) = output else {
        return Ok(Vec::new());
    };

    if !output.status.success() {
        return Ok(Vec::new());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let files = stdout
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| workspace.join(line))
        .collect();

    Ok(files)
}

fn write_postings(
    path: &Path,
    repo_id: &str,
    commit_id: &str,
    docs: &[DocumentMeta],
    sorted_hashes: &[u64],
    buckets: &HashMap<u64, Vec<u32>>,
    bigram_frequency: &HashMap<u16, u32>,
) -> Result<Vec<(u64, u64)>> {
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .read(true)
        .open(path)?;

    file.write_all(&[0u8; POSTINGS_HEADER_SIZE])?;

    let doc_table_offset = POSTINGS_HEADER_SIZE as u64;

    for doc in docs {
        write_u32_to_writer(&mut file, doc.doc_id)?;
        let path_bytes = doc.path.as_bytes();
        if path_bytes.len() > u16::MAX as usize {
            return Err(FastRegexError::Internal(format!(
                "path too long for index encoding: {}",
                doc.path
            )));
        }

        write_u16_to_writer(&mut file, path_bytes.len() as u16)?;
        file.write_all(path_bytes)?;
        write_u64_to_writer(&mut file, doc.size)?;
        write_i64_to_writer(&mut file, doc.mtime_secs)?;
        file.write_all(&[u8::from(doc.indexed)])?;
    }

    let postings_offset = file.stream_position()?;
    let mut lookup_entries = Vec::<(u64, u64)>::with_capacity(sorted_hashes.len());

    for hash in sorted_hashes {
        let Some(doc_ids) = buckets.get(hash) else {
            continue;
        };

        let offset = file.stream_position()?;
        lookup_entries.push((*hash, offset));

        write_u32_to_writer(&mut file, doc_ids.len() as u32)?;
        for doc_id in doc_ids {
            write_u32_to_writer(&mut file, *doc_id)?;
        }
    }

    let bigram_offset = file.stream_position()?;
    write_u32_to_writer(&mut file, bigram_frequency.len() as u32)?;

    let mut pairs: Vec<(u16, u32)> = bigram_frequency.iter().map(|(k, v)| (*k, *v)).collect();
    pairs.sort_unstable_by_key(|(k, _)| *k);

    for (key, freq) in pairs {
        write_u16_to_writer(&mut file, key)?;
        write_u32_to_writer(&mut file, freq)?;
    }

    file.flush()?;
    drop(file);

    let checksum = compute_payload_checksum(path, POSTINGS_HEADER_SIZE as u64)?;

    let header = PostingsHeader {
        checksum,
        doc_table_offset,
        postings_offset,
        bigram_offset,
        doc_count: docs.len() as u32,
        posting_count: lookup_entries.len() as u32,
        repo_id: repo_id.to_string(),
        commit_id: commit_id.to_string(),
    };

    rewrite_postings_header(path, &header)?;

    Ok(lookup_entries)
}

fn rewrite_postings_header(path: &Path, header: &PostingsHeader) -> Result<()> {
    let mut raw = [0u8; POSTINGS_HEADER_SIZE];

    raw[0..8].copy_from_slice(POSTINGS_MAGIC);
    raw[8..12].copy_from_slice(&POSTINGS_VERSION.to_le_bytes());
    raw[12..20].copy_from_slice(&header.checksum.to_le_bytes());
    raw[20..28].copy_from_slice(&header.doc_table_offset.to_le_bytes());
    raw[28..36].copy_from_slice(&header.postings_offset.to_le_bytes());
    raw[36..44].copy_from_slice(&header.bigram_offset.to_le_bytes());
    raw[44..48].copy_from_slice(&header.doc_count.to_le_bytes());
    raw[48..52].copy_from_slice(&header.posting_count.to_le_bytes());

    let repo_bytes = header.repo_id.as_bytes();
    let commit_bytes = header.commit_id.as_bytes();

    let repo_len = repo_bytes.len().min(64) as u16;
    let commit_len = commit_bytes.len().min(64) as u16;
    raw[52..54].copy_from_slice(&repo_len.to_le_bytes());
    raw[54..56].copy_from_slice(&commit_len.to_le_bytes());

    raw[56..(56 + repo_len as usize)].copy_from_slice(&repo_bytes[..repo_len as usize]);
    raw[120..(120 + commit_len as usize)].copy_from_slice(&commit_bytes[..commit_len as usize]);

    let mut file = OpenOptions::new().write(true).open(path)?;
    file.seek(SeekFrom::Start(0))?;
    file.write_all(&raw)?;
    file.flush()?;

    Ok(())
}

fn write_lookup(path: &Path, entries: &[(u64, u64)]) -> Result<()> {
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .read(true)
        .open(path)?;

    file.write_all(&[0u8; LOOKUP_HEADER_SIZE])?;

    for (hash, offset) in entries {
        write_u64_to_writer(&mut file, *hash)?;
        write_u64_to_writer(&mut file, *offset)?;
    }

    file.flush()?;
    drop(file);

    let checksum = compute_payload_checksum(path, LOOKUP_HEADER_SIZE as u64)?;
    rewrite_lookup_header(
        path,
        &LookupHeader {
            checksum,
            entry_count: entries.len() as u32,
        },
    )
}

fn rewrite_lookup_header(path: &Path, header: &LookupHeader) -> Result<()> {
    let mut raw = [0u8; LOOKUP_HEADER_SIZE];

    raw[0..8].copy_from_slice(LOOKUP_MAGIC);
    raw[8..12].copy_from_slice(&LOOKUP_VERSION.to_le_bytes());
    raw[12..20].copy_from_slice(&header.checksum.to_le_bytes());
    raw[20..24].copy_from_slice(&header.entry_count.to_le_bytes());

    let mut file = OpenOptions::new().write(true).open(path)?;
    file.seek(SeekFrom::Start(0))?;
    file.write_all(&raw)?;
    file.flush()?;

    Ok(())
}

fn decode_postings_header(bytes: &[u8]) -> Result<PostingsHeader> {
    if bytes.len() < POSTINGS_HEADER_SIZE {
        return Err(FastRegexError::CorruptIndex(
            "postings.bin smaller than header".to_string(),
        ));
    }

    if &bytes[0..8] != POSTINGS_MAGIC {
        return Err(FastRegexError::CorruptIndex(
            "postings.bin magic mismatch".to_string(),
        ));
    }

    let version = read_u32(bytes, 8)?;
    if version != POSTINGS_VERSION {
        return Err(FastRegexError::CorruptIndex(format!(
            "unsupported postings version {version}"
        )));
    }

    let checksum = read_u64(bytes, 12)?;
    let doc_table_offset = read_u64(bytes, 20)?;
    let postings_offset = read_u64(bytes, 28)?;
    let bigram_offset = read_u64(bytes, 36)?;
    let doc_count = read_u32(bytes, 44)?;
    let posting_count = read_u32(bytes, 48)?;

    let repo_len = read_u16(bytes, 52)? as usize;
    let commit_len = read_u16(bytes, 54)? as usize;

    if repo_len > 64 || commit_len > 64 {
        return Err(FastRegexError::CorruptIndex(
            "invalid repo/commit length in postings header".to_string(),
        ));
    }

    let repo_id = String::from_utf8_lossy(&bytes[56..(56 + repo_len)]).to_string();
    let commit_id = String::from_utf8_lossy(&bytes[120..(120 + commit_len)]).to_string();

    Ok(PostingsHeader {
        checksum,
        doc_table_offset,
        postings_offset,
        bigram_offset,
        doc_count,
        posting_count,
        repo_id,
        commit_id,
    })
}

fn decode_lookup_header(bytes: &[u8]) -> Result<LookupHeader> {
    if bytes.len() < LOOKUP_HEADER_SIZE {
        return Err(FastRegexError::CorruptIndex(
            "lookup.bin smaller than header".to_string(),
        ));
    }

    if &bytes[0..8] != LOOKUP_MAGIC {
        return Err(FastRegexError::CorruptIndex(
            "lookup.bin magic mismatch".to_string(),
        ));
    }

    let version = read_u32(bytes, 8)?;
    if version != LOOKUP_VERSION {
        return Err(FastRegexError::CorruptIndex(format!(
            "unsupported lookup version {version}"
        )));
    }

    let checksum = read_u64(bytes, 12)?;
    let entry_count = read_u32(bytes, 20)?;

    Ok(LookupHeader {
        checksum,
        entry_count,
    })
}

fn decode_doc_table(bytes: &[u8], header: &PostingsHeader) -> Result<Vec<DocumentMeta>> {
    let mut cursor = header.doc_table_offset as usize;
    let postings_offset = header.postings_offset as usize;

    if cursor > bytes.len() || postings_offset > bytes.len() || cursor > postings_offset {
        return Err(FastRegexError::CorruptIndex(
            "invalid doc_table/postings offsets".to_string(),
        ));
    }

    let mut docs = Vec::with_capacity(header.doc_count as usize);

    for _ in 0..header.doc_count {
        let doc_id = read_u32(bytes, cursor)?;
        cursor += 4;

        let path_len = read_u16(bytes, cursor)? as usize;
        cursor += 2;

        if cursor + path_len > postings_offset {
            return Err(FastRegexError::CorruptIndex(
                "doc table path out of bounds".to_string(),
            ));
        }

        let path = String::from_utf8_lossy(&bytes[cursor..cursor + path_len]).to_string();
        cursor += path_len;

        let size = read_u64(bytes, cursor)?;
        cursor += 8;

        let mtime_secs = read_i64(bytes, cursor)?;
        cursor += 8;

        let indexed = read_u8(bytes, cursor)? != 0;
        cursor += 1;

        docs.push(DocumentMeta {
            doc_id,
            path,
            size,
            mtime_secs,
            indexed,
        });
    }

    Ok(docs)
}

fn decode_bigram_table(bytes: &[u8], header: &PostingsHeader) -> Result<HashMap<u16, u32>> {
    let mut cursor = header.bigram_offset as usize;
    if cursor + 4 > bytes.len() {
        return Err(FastRegexError::CorruptIndex(
            "bigram table offset out of bounds".to_string(),
        ));
    }

    let count = read_u32(bytes, cursor)? as usize;
    cursor += 4;

    let mut out = HashMap::with_capacity(count);
    for _ in 0..count {
        let key = read_u16(bytes, cursor)?;
        cursor += 2;
        let freq = read_u32(bytes, cursor)?;
        cursor += 4;
        out.insert(key, freq);
    }

    Ok(out)
}

fn validate_payload_checksum(
    bytes: &[u8],
    payload_offset: usize,
    expected: u64,
    label: &str,
) -> Result<()> {
    if payload_offset > bytes.len() {
        return Err(FastRegexError::CorruptIndex(format!(
            "{label} payload offset out of bounds"
        )));
    }

    let digest = blake3::hash(&bytes[payload_offset..]);
    let actual = u64::from_le_bytes(digest.as_bytes()[0..8].try_into().unwrap());

    if actual != expected {
        return Err(FastRegexError::CorruptIndex(format!(
            "{label} checksum mismatch"
        )));
    }

    Ok(())
}

fn compute_payload_checksum(path: &Path, payload_offset: u64) -> Result<u64> {
    let mut file = File::open(path)?;
    file.seek(SeekFrom::Start(payload_offset))?;

    let mut hasher = blake3::Hasher::new();
    let mut buf = [0u8; 64 * 1024];

    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }

    let digest = hasher.finalize();
    Ok(u64::from_le_bytes(
        digest.as_bytes()[0..8].try_into().unwrap(),
    ))
}

fn write_u16_to_writer<W: Write>(writer: &mut W, value: u16) -> Result<()> {
    writer.write_all(&value.to_le_bytes())?;
    Ok(())
}

fn write_u32_to_writer<W: Write>(writer: &mut W, value: u32) -> Result<()> {
    writer.write_all(&value.to_le_bytes())?;
    Ok(())
}

fn write_u64_to_writer<W: Write>(writer: &mut W, value: u64) -> Result<()> {
    writer.write_all(&value.to_le_bytes())?;
    Ok(())
}

fn write_i64_to_writer<W: Write>(writer: &mut W, value: i64) -> Result<()> {
    writer.write_all(&value.to_le_bytes())?;
    Ok(())
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16> {
    let end = offset + 2;
    if end > bytes.len() {
        return Err(FastRegexError::CorruptIndex(
            "u16 read out of bounds".to_string(),
        ));
    }
    Ok(u16::from_le_bytes(bytes[offset..end].try_into().unwrap()))
}

fn read_u8(bytes: &[u8], offset: usize) -> Result<u8> {
    if offset >= bytes.len() {
        return Err(FastRegexError::CorruptIndex(
            "u8 read out of bounds".to_string(),
        ));
    }
    Ok(bytes[offset])
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32> {
    let end = offset + 4;
    if end > bytes.len() {
        return Err(FastRegexError::CorruptIndex(
            "u32 read out of bounds".to_string(),
        ));
    }
    Ok(u32::from_le_bytes(bytes[offset..end].try_into().unwrap()))
}

fn read_i64(bytes: &[u8], offset: usize) -> Result<i64> {
    let end = offset + 8;
    if end > bytes.len() {
        return Err(FastRegexError::CorruptIndex(
            "i64 read out of bounds".to_string(),
        ));
    }
    Ok(i64::from_le_bytes(bytes[offset..end].try_into().unwrap()))
}

fn read_u64(bytes: &[u8], offset: usize) -> Result<u64> {
    let end = offset + 8;
    if end > bytes.len() {
        return Err(FastRegexError::CorruptIndex(
            "u64 read out of bounds".to_string(),
        ));
    }
    Ok(u64::from_le_bytes(bytes[offset..end].try_into().unwrap()))
}
