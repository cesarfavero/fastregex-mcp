use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Copy)]
pub struct SparseConfig {
    pub max_span_bigrams: usize,
    pub max_grams_per_text: usize,
    pub max_covering_grams: usize,
}

impl Default for SparseConfig {
    fn default() -> Self {
        Self {
            max_span_bigrams: 24,
            max_grams_per_text: 40_000,
            max_covering_grams: 12,
        }
    }
}

#[derive(Debug, Clone)]
struct SparseGram {
    start: usize,
    end: usize,
    bytes: Vec<u8>,
    score: u32,
}

#[inline]
fn bigram_key(a: u8, b: u8) -> u16 {
    ((a as u16) << 8) | b as u16
}

#[inline]
fn rarity_weight(freq: &HashMap<u16, u32>, max_freq: u32, a: u8, b: u8) -> u32 {
    let seen = *freq.get(&bigram_key(a, b)).unwrap_or(&0);
    max_freq.saturating_sub(seen).saturating_add(1)
}

fn compute_weights(text: &[u8], freq: &HashMap<u16, u32>) -> Vec<u32> {
    if text.len() < 2 {
        return Vec::new();
    }

    let max_freq = freq.values().copied().max().unwrap_or(1);
    text.windows(2)
        .map(|bg| rarity_weight(freq, max_freq, bg[0], bg[1]))
        .collect()
}

fn build_all_with_spans(
    text: &[u8],
    freq: &HashMap<u16, u32>,
    cfg: SparseConfig,
) -> Vec<SparseGram> {
    if text.len() < 3 {
        return Vec::new();
    }

    let weights = compute_weights(text, freq);
    if weights.len() < 2 {
        return Vec::new();
    }

    let mut out = Vec::new();
    let mut seen = HashSet::<Vec<u8>>::new();

    for l in 0..weights.len() {
        let mut max_inside = 0u32;
        let max_r = usize::min(
            weights.len() - 1,
            l.saturating_add(cfg.max_span_bigrams.saturating_sub(1)),
        );

        for r in (l + 1)..=max_r {
            if r > l + 1 {
                max_inside = max_inside.max(weights[r - 1]);
            }

            if weights[l] > max_inside && weights[r] > max_inside {
                let start = l;
                let end = r + 2;
                if end <= text.len() && (end - start) >= 3 {
                    let gram = text[start..end].to_vec();
                    if seen.insert(gram.clone()) {
                        let score = weights[l].min(weights[r]);
                        out.push(SparseGram {
                            start,
                            end,
                            bytes: gram,
                            score,
                        });

                        if out.len() >= cfg.max_grams_per_text {
                            return out;
                        }
                    }
                }
            }
        }
    }

    out
}

pub fn build_all_sparse_ngrams(
    text: &[u8],
    freq: &HashMap<u16, u32>,
    cfg: SparseConfig,
) -> Vec<Vec<u8>> {
    build_all_with_spans(text, freq, cfg)
        .into_iter()
        .map(|g| g.bytes)
        .collect()
}

pub fn build_covering_sparse_ngrams(
    literal: &[u8],
    freq: &HashMap<u16, u32>,
    cfg: SparseConfig,
) -> Vec<Vec<u8>> {
    if literal.len() < 3 {
        return Vec::new();
    }

    let mut grams = build_all_with_spans(literal, freq, cfg);
    if grams.is_empty() {
        return vec![literal[0..3].to_vec()];
    }

    grams.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then((b.end - b.start).cmp(&(a.end - a.start)))
    });

    let mut covered = vec![false; literal.len()];
    let mut selected = Vec::<SparseGram>::new();

    for gram in grams {
        if selected.len() >= cfg.max_covering_grams {
            break;
        }

        let improves_coverage = (gram.start..gram.end).any(|idx| !covered[idx]);
        if !improves_coverage {
            continue;
        }

        for idx in gram.start..gram.end {
            covered[idx] = true;
        }

        selected.push(gram);

        if covered.iter().all(|c| *c) {
            break;
        }
    }

    if selected.is_empty() {
        return vec![literal[0..3].to_vec()];
    }

    let mut out = Vec::<Vec<u8>>::new();
    let mut dedup = HashSet::<Vec<u8>>::new();

    for gram in selected {
        if dedup.insert(gram.bytes.clone()) {
            out.push(gram.bytes);
        }
    }

    out
}

pub fn build_bigram_frequency(text: &[u8], table: &mut HashMap<u16, u32>) {
    if text.len() < 2 {
        return;
    }

    for bg in text.windows(2) {
        let key = bigram_key(bg[0], bg[1]);
        let next = table.get(&key).copied().unwrap_or(0).saturating_add(1);
        table.insert(key, next);
    }
}
