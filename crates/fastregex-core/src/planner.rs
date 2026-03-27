use std::collections::{HashMap, HashSet};

use crate::hashing::hash_gram;
use crate::sparse::{SparseConfig, build_covering_sparse_ngrams};

#[derive(Debug, Clone)]
pub enum PlanExpr {
    AllDocs,
    And(Vec<u64>),
    Or(Vec<Vec<u64>>),
}

#[derive(Debug, Clone)]
pub struct QueryPlan {
    pub expr: PlanExpr,
    pub extracted_literals: Vec<String>,
    pub used_fallback: bool,
}

impl QueryPlan {
    pub fn all_docs() -> Self {
        Self {
            expr: PlanExpr::AllDocs,
            extracted_literals: Vec::new(),
            used_fallback: true,
        }
    }

    pub fn matches_hashes(&self, hashes: &HashSet<u64>) -> bool {
        match &self.expr {
            PlanExpr::AllDocs => true,
            PlanExpr::And(required) => required.iter().all(|h| hashes.contains(h)),
            PlanExpr::Or(branches) => branches
                .iter()
                .any(|branch| branch.iter().all(|h| hashes.contains(h))),
        }
    }
}

pub fn build_query_plan(
    pattern: &str,
    bigram_frequency: &HashMap<u16, u32>,
    sparse_cfg: SparseConfig,
) -> QueryPlan {
    if pattern.is_empty() {
        return QueryPlan::all_docs();
    }

    let branches = split_top_level_alternation(pattern);
    if branches.is_empty() {
        return QueryPlan::all_docs();
    }

    let mut literal_branches = Vec::<String>::with_capacity(branches.len());
    let mut hashed_branches = Vec::<Vec<u64>>::with_capacity(branches.len());

    for branch in branches {
        let analysis = analyze_branch_required_literals(&branch);
        if !analysis.parse_ok {
            return QueryPlan::all_docs();
        }

        let mut dedup = HashSet::<u64>::new();
        let mut branch_literals = Vec::<String>::new();

        for literal in analysis.required_literals {
            if literal.len() < 3 {
                continue;
            }

            let grams = build_covering_sparse_ngrams(literal.as_bytes(), bigram_frequency, sparse_cfg);
            for gram in grams {
                if gram.len() == 3 {
                    dedup.insert(hash_gram(&gram));
                }
            }
            if dedup.len() < MIN_QUERY_GRAMS {
                let limit = if literal.len() >= 12 {
                    MAX_TRIGRAM_ADDS
                } else {
                    MAX_TRIGRAM_ADDS / 2
                };
                add_literal_trigrams(literal.as_bytes(), &mut dedup, limit);
            }
            branch_literals.push(literal);
        }

        if dedup.is_empty() {
            return QueryPlan::all_docs();
        }

        let mut hashes: Vec<u64> = dedup.into_iter().collect();
        hashes.sort_unstable();
        hashed_branches.push(hashes);
        literal_branches.extend(branch_literals);
    }

    let expr = if hashed_branches.len() == 1 {
        PlanExpr::And(hashed_branches.remove(0))
    } else {
        PlanExpr::Or(hashed_branches)
    };

    QueryPlan {
        expr,
        extracted_literals: literal_branches,
        used_fallback: false,
    }
}

const MIN_QUERY_GRAMS: usize = 3;
const MAX_TRIGRAM_ADDS: usize = 16;

#[derive(Debug, Clone)]
struct BranchLiteralAnalysis {
    required_literals: Vec<String>,
    parse_ok: bool,
}

fn analyze_branch_required_literals(branch: &str) -> BranchLiteralAnalysis {
    if branch.is_empty() {
        return BranchLiteralAnalysis {
            required_literals: Vec::new(),
            parse_ok: true,
        };
    }

    let chars: Vec<char> = branch.chars().collect();
    let mut i = 0usize;
    let mut current = String::new();
    let mut literals = Vec::<String>::new();
    let mut parse_ok = true;

    while i < chars.len() {
        let ch = chars[i];

        let atom = match ch {
            '\\' => parse_escape_atom(&chars, i),
            '[' => parse_character_class_atom(&chars, i),
            '(' => parse_group_atom(&chars, i),
            ')' | '|' => {
                // Unexpected unmatched separators in a pre-split branch:
                // keep parser running conservatively as wildcard.
                AtomParse {
                    kind: AtomKind::Wildcard,
                    next: i + 1,
                    parse_ok: true,
                }
            }
            '.' => AtomParse {
                kind: AtomKind::Wildcard,
                next: i + 1,
                parse_ok: true,
            },
            '^' | '$' => AtomParse {
                kind: AtomKind::ZeroWidth,
                next: i + 1,
                parse_ok: true,
            },
            '?' | '*' | '+' | '{' => AtomParse {
                // Standalone quantifier-like tokens are treated as wildcards.
                kind: AtomKind::Wildcard,
                next: i + 1,
                parse_ok: true,
            },
            _ => AtomParse {
                kind: AtomKind::Literal(ch),
                next: i + 1,
                parse_ok: true,
            },
        };

        parse_ok &= atom.parse_ok;
        i = atom.next;

        let (min_repeat, next_after_quant, quant_ok) = parse_quantifier(&chars, i);
        parse_ok &= quant_ok;
        i = next_after_quant;

        match atom.kind {
            AtomKind::Literal(c) => {
                if min_repeat == 0 {
                    flush_literal(&mut current, &mut literals);
                } else {
                    let repeat = min_repeat.min(8);
                    for _ in 0..repeat {
                        current.push(c);
                    }
                }
            }
            AtomKind::Wildcard | AtomKind::ZeroWidth => {
                flush_literal(&mut current, &mut literals);
            }
        }
    }

    flush_literal(&mut current, &mut literals);

    BranchLiteralAnalysis {
        required_literals: literals,
        parse_ok,
    }
}

fn add_literal_trigrams(literal: &[u8], dedup: &mut HashSet<u64>, limit: usize) {
    if literal.len() < 3 || limit == 0 {
        return;
    }

    let mut added = 0usize;
    for tri in literal.windows(3) {
        if dedup.insert(hash_gram(tri)) {
            added += 1;
            if added >= limit {
                break;
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum AtomKind {
    Literal(char),
    Wildcard,
    ZeroWidth,
}

#[derive(Debug, Clone, Copy)]
struct AtomParse {
    kind: AtomKind,
    next: usize,
    parse_ok: bool,
}

fn parse_escape_atom(chars: &[char], start: usize) -> AtomParse {
    if start + 1 >= chars.len() {
        return AtomParse {
            kind: AtomKind::Wildcard,
            next: chars.len(),
            parse_ok: false,
        };
    }

    let escaped = chars[start + 1];
    let kind = match escaped {
        // Escaped metacharacters are literals.
        '.' | '^' | '$' | '*' | '+' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '|' | '\\' => {
            AtomKind::Literal(escaped)
        }
        // Common escaped literal control chars.
        'n' => AtomKind::Literal('\n'),
        'r' => AtomKind::Literal('\r'),
        't' => AtomKind::Literal('\t'),
        // Word boundary and similar assertions.
        'b' | 'B' | 'A' | 'z' | 'Z' | 'G' => AtomKind::ZeroWidth,
        // Classes/backrefs/other escapes remain wildcard.
        _ => AtomKind::Wildcard,
    };

    AtomParse {
        kind,
        next: start + 2,
        parse_ok: true,
    }
}

fn parse_character_class_atom(chars: &[char], start: usize) -> AtomParse {
    let mut i = start + 1;
    let mut escaped = false;

    while i < chars.len() {
        let ch = chars[i];
        if escaped {
            escaped = false;
            i += 1;
            continue;
        }

        match ch {
            '\\' => {
                escaped = true;
                i += 1;
            }
            ']' => {
                return AtomParse {
                    kind: AtomKind::Wildcard,
                    next: i + 1,
                    parse_ok: true,
                };
            }
            _ => i += 1,
        }
    }

    AtomParse {
        kind: AtomKind::Wildcard,
        next: chars.len(),
        parse_ok: false,
    }
}

fn parse_group_atom(chars: &[char], start: usize) -> AtomParse {
    let mut i = start + 1;
    let mut depth = 1usize;
    let mut escaped = false;
    let mut class_depth = 0usize;

    while i < chars.len() {
        let ch = chars[i];
        if escaped {
            escaped = false;
            i += 1;
            continue;
        }

        match ch {
            '\\' => {
                escaped = true;
                i += 1;
            }
            '[' => {
                class_depth += 1;
                i += 1;
            }
            ']' => {
                class_depth = class_depth.saturating_sub(1);
                i += 1;
            }
            '(' if class_depth == 0 => {
                depth += 1;
                i += 1;
            }
            ')' if class_depth == 0 => {
                depth = depth.saturating_sub(1);
                i += 1;
                if depth == 0 {
                    return AtomParse {
                        kind: AtomKind::Wildcard,
                        next: i,
                        parse_ok: true,
                    };
                }
            }
            _ => i += 1,
        }
    }

    AtomParse {
        kind: AtomKind::Wildcard,
        next: chars.len(),
        parse_ok: false,
    }
}

fn parse_quantifier(chars: &[char], start: usize) -> (usize, usize, bool) {
    if start >= chars.len() {
        return (1, start, true);
    }

    match chars[start] {
        '?' => (0, start + 1, true),
        '*' => (0, start + 1, true),
        '+' => (1, start + 1, true),
        '{' => parse_brace_quantifier(chars, start),
        _ => (1, start, true),
    }
}

fn parse_brace_quantifier(chars: &[char], start: usize) -> (usize, usize, bool) {
    let mut i = start + 1;
    let mut body = String::new();

    while i < chars.len() {
        if chars[i] == '}' {
            break;
        }
        body.push(chars[i]);
        i += 1;
    }

    if i >= chars.len() || chars[i] != '}' {
        return (1, start, false);
    }

    let next = i + 1;
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return (1, start, false);
    }

    let min = if let Some((left, _right)) = trimmed.split_once(',') {
        left.trim().parse::<usize>().ok()
    } else {
        trimmed.parse::<usize>().ok()
    };

    match min {
        Some(value) => (value, next, true),
        None => (1, start, false),
    }
}

fn flush_literal(current: &mut String, out: &mut Vec<String>) {
    if current.is_empty() {
        return;
    }

    out.push(std::mem::take(current));
}

fn split_top_level_alternation(pattern: &str) -> Vec<String> {
    let mut branches = Vec::<String>::new();
    let mut current = String::new();

    let mut escaped = false;
    let mut class_depth = 0usize;
    let mut paren_depth = 0usize;

    for ch in pattern.chars() {
        if escaped {
            current.push(ch);
            escaped = false;
            continue;
        }

        match ch {
            '\\' => {
                current.push(ch);
                escaped = true;
            }
            '[' => {
                class_depth = class_depth.saturating_add(1);
                current.push(ch);
            }
            ']' => {
                class_depth = class_depth.saturating_sub(1);
                current.push(ch);
            }
            '(' if class_depth == 0 => {
                paren_depth = paren_depth.saturating_add(1);
                current.push(ch);
            }
            ')' if class_depth == 0 => {
                paren_depth = paren_depth.saturating_sub(1);
                current.push(ch);
            }
            '|' if class_depth == 0 && paren_depth == 0 => {
                branches.push(current);
                current = String::new();
            }
            _ => current.push(ch),
        }
    }

    branches.push(current);
    branches
}
