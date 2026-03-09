use anyhow::{Context, Result, bail};
use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Clone, Debug)]
pub struct SearchCandidate {
    pub path: PathBuf,
    pub name: String,
    pub name_key: String,
    pub relative: String,
    pub relative_key: String,
    pub is_dir: bool,
}

pub fn collect_candidates(cwd: &Path, show_hidden: bool) -> Result<Vec<SearchCandidate>> {
    let mut command = Command::new("fd");
    command.args([".", "--strip-cwd-prefix", "--follow", "--exclude", ".git"]);
    if show_hidden {
        command.arg("--hidden");
    }
    command.current_dir(cwd);

    let output = command.output().context("failed to run fd")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if stderr.is_empty() {
            bail!("fd exited with status {}", output.status);
        }
        bail!("fd failed: {stderr}");
    }

    let mut candidates = Vec::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let relative = line.trim();
        if relative.is_empty() {
            continue;
        }

        let path = cwd.join(relative);
        let is_dir = path.is_dir();
        let name = Path::new(relative)
            .file_name()
            .and_then(OsStr::to_str)
            .map(str::to_string)
            .unwrap_or_else(|| relative.to_string());
        let name_key = name.to_lowercase();
        let relative_key = relative.to_lowercase();

        candidates.push(SearchCandidate {
            path,
            name,
            name_key,
            relative: relative.to_string(),
            relative_key,
            is_dir,
        });
    }

    candidates.sort_by(|left, right| {
        right
            .is_dir
            .cmp(&left.is_dir)
            .then_with(|| left.relative_key.cmp(&right.relative_key))
    });
    Ok(candidates)
}

pub fn filter_candidates_in<I>(
    candidates: &[SearchCandidate],
    pool: I,
    query: &str,
    limit: usize,
) -> Vec<usize>
where
    I: IntoIterator<Item = usize>,
{
    if query.trim().is_empty() {
        return pool.into_iter().take(limit).collect();
    }

    let needle = query.to_lowercase().into_bytes();
    let mut top = Vec::<(usize, i64, usize)>::with_capacity(limit.min(64));

    for index in pool {
        let candidate = &candidates[index];
        let name_score = fuzzy_score_bytes(&needle, candidate.name_key.as_bytes())
            .map(|score| score + 80 + i64::from(candidate.is_dir) * 12);
        let path_score = fuzzy_score_bytes(&needle, candidate.relative_key.as_bytes());
        let score = match (name_score, path_score) {
            (Some(name), Some(path)) => name.max(path),
            (Some(name), None) => name,
            (None, Some(path)) => path,
            (None, None) => continue,
        };

        let entry = (index, score, candidate.relative.len());
        let insert_at = top
            .binary_search_by(|existing| compare_scored(candidates, existing, &entry))
            .unwrap_or_else(|slot| slot);

        if insert_at >= limit {
            continue;
        }

        top.insert(insert_at, entry);
        if top.len() > limit {
            top.pop();
        }
    }

    top.into_iter().map(|(index, _, _)| index).collect()
}

fn compare_scored(
    candidates: &[SearchCandidate],
    left: &(usize, i64, usize),
    right: &(usize, i64, usize),
) -> std::cmp::Ordering {
    right
        .1
        .cmp(&left.1)
        .then_with(|| left.2.cmp(&right.2))
        .then_with(|| {
            candidates[left.0]
                .relative_key
                .cmp(&candidates[right.0].relative_key)
        })
}

fn fuzzy_score_bytes(query: &[u8], text: &[u8]) -> Option<i64> {
    if query.is_empty() {
        return Some(0);
    }
    if text.is_empty() {
        return None;
    }

    let mut score = 0i64;
    let mut scan_at = 0usize;
    let mut last_match = None;
    let mut streak = 0i64;

    for &byte in query {
        let mut found = None;
        for (index, &candidate) in text.iter().enumerate().skip(scan_at) {
            if candidate == byte {
                found = Some(index);
                break;
            }
        }
        let index = found?;

        if index == 0
            || matches!(
                text[index.saturating_sub(1)],
                b'/' | b'-' | b'_' | b' ' | b'.'
            )
        {
            score += 18;
        }

        if let Some(previous) = last_match {
            if index == previous + 1 {
                streak += 1;
                score += 20 + streak * 6;
            } else {
                streak = 0;
                score -= (index - previous - 1) as i64;
            }
        } else {
            score += 12;
            score -= index as i64;
        }

        score += 10;
        scan_at = index + 1;
        last_match = Some(index);
    }

    score -= (text.len().saturating_sub(scan_at)) as i64 / 3;
    Some(score)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzzy_filter_prefers_tighter_name_match() {
        let candidates = vec![
            SearchCandidate {
                path: PathBuf::from("/tmp/src/main.rs"),
                name: "main.rs".to_string(),
                name_key: "main.rs".to_string(),
                relative: "src/main.rs".to_string(),
                relative_key: "src/main.rs".to_string(),
                is_dir: false,
            },
            SearchCandidate {
                path: PathBuf::from("/tmp/docs/readme.md"),
                name: "readme.md".to_string(),
                name_key: "readme.md".to_string(),
                relative: "docs/readme.md".to_string(),
                relative_key: "docs/readme.md".to_string(),
                is_dir: false,
            },
        ];

        let matches = filter_candidates_in(&candidates, 0..candidates.len(), "mn", 10);
        assert_eq!(matches.first().copied(), Some(0));
    }
}
