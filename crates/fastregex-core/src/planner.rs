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
        let Some(literal) = parse_pure_literal(&branch) else {
            return QueryPlan::all_docs();
        };

        if literal.len() < 3 {
            return QueryPlan::all_docs();
        }

        let grams = build_covering_sparse_ngrams(literal.as_bytes(), bigram_frequency, sparse_cfg);
        let mut dedup = HashSet::<u64>::new();

        for gram in grams {
            dedup.insert(hash_gram(&gram));
        }

        if dedup.is_empty() {
            return QueryPlan::all_docs();
        }

        let mut hashes: Vec<u64> = dedup.into_iter().collect();
        hashes.sort_unstable();
        hashed_branches.push(hashes);
        literal_branches.push(literal);
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

fn parse_pure_literal(branch: &str) -> Option<String> {
    if branch.is_empty() {
        return None;
    }

    let mut out = String::new();
    let mut chars = branch.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '\\' {
            let next = chars.next()?;
            if is_escaped_literal(next) {
                out.push(next);
                continue;
            }

            // Unknown escapes can carry semantic regex meaning (e.g. \d, \b),
            // so we do not index them as mandatory literals.
            return None;
        }

        if is_regex_meta(ch) {
            return None;
        }

        out.push(ch);
    }

    if out.is_empty() { None } else { Some(out) }
}

#[inline]
fn is_regex_meta(ch: char) -> bool {
    matches!(
        ch,
        '.' | '^' | '$' | '*' | '+' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '|'
    )
}

#[inline]
fn is_escaped_literal(ch: char) -> bool {
    matches!(
        ch,
        '.' | '^' | '$' | '*' | '+' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '|' | '\\'
    )
}
