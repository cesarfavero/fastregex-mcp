use globset::{Glob, GlobSet, GlobSetBuilder};

use crate::error::{FastRegexError, Result};

#[derive(Debug, Clone)]
pub struct PathFilter {
    include: Option<GlobSet>,
    exclude: Option<GlobSet>,
}

impl PathFilter {
    pub fn new(include: &[String], extra_globs: &[String], exclude: &[String]) -> Result<Self> {
        let include = compile_set(include.iter().chain(extra_globs.iter()))?;
        let exclude = compile_set(exclude.iter())?;

        Ok(Self { include, exclude })
    }

    pub fn allows(&self, path: &str) -> bool {
        let include_ok = match &self.include {
            Some(set) => set.is_match(path),
            None => true,
        };

        if !include_ok {
            return false;
        }

        if let Some(set) = &self.exclude {
            if set.is_match(path) {
                return false;
            }
        }

        true
    }
}

fn compile_set<'a, I>(patterns: I) -> Result<Option<GlobSet>>
where
    I: Iterator<Item = &'a String>,
{
    let mut builder = GlobSetBuilder::new();
    let mut count = 0usize;

    for pattern in patterns {
        let glob = Glob::new(pattern).map_err(|e| FastRegexError::Glob(e.to_string()))?;
        builder.add(glob);
        count += 1;
    }

    if count == 0 {
        return Ok(None);
    }

    let set = builder
        .build()
        .map_err(|e| FastRegexError::Glob(e.to_string()))?;

    Ok(Some(set))
}
