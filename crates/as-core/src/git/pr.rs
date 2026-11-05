//! Read-only pull request review through the GitHub CLI.

use std::path::Path;

use serde_json::Value;

use super::error::{GitError, GitResult};
use super::runner::{GitRunner, args};
use super::types::{GitPrDiff, GitPrFile, GitPrMetadata};
use super::unified_diff::{
    DiffFileStatus, extract_file_diff, parse_unified_diff, split_diff_by_file,
};

const PR_FIELDS: &str = "number,title,author,state,baseRefName,headRefName,body,url,additions,deletions,changedFiles,createdAt,updatedAt";

/// What to hand to `gh pr ...`: a PR URL as is (it names its repository), else the number.
pub fn parse_pr_identifier(identifier: &str) -> GitResult<String> {
    let id = identifier.trim();
    if let Some(pos) = id.find("/pull/") {
        let digits: String = id[pos + 6..]
            .chars()
            .take_while(char::is_ascii_digit)
            .collect();
        if digits.is_empty() {
            return Err(GitError::InvalidInput(format!(
                "Invalid PR identifier: {identifier}"
            )));
        }
        if id.starts_with("http://") || id.starts_with("https://") {
            return Ok(id.to_string());
        }
        return Ok(digits);
    }
    let cleaned = id.trim_start_matches('#').trim();
    match cleaned.parse::<u64>() {
        Ok(n) => Ok(n.to_string()),
        Err(_) => Err(GitError::InvalidInput(format!(
            "Invalid PR identifier: {identifier}"
        ))),
    }
}

fn s(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn n(v: &Value, key: &str) -> u64 {
    v.get(key).and_then(Value::as_u64).unwrap_or(0)
}

/// Tolerant parse of `gh pr view --json ...` output.
pub fn parse_pr_metadata(json: &str) -> GitResult<GitPrMetadata> {
    let v: Value = serde_json::from_str(json).map_err(|e| GitError::Parse(e.to_string()))?;
    let author = v
        .get("author")
        .and_then(|a| a.get("login").or_else(|| a.get("name")))
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string();
    Ok(GitPrMetadata {
        number: n(&v, "number"),
        title: s(&v, "title"),
        author,
        state: s(&v, "state"),
        base_ref: s(&v, "baseRefName"),
        head_ref: s(&v, "headRefName"),
        body: s(&v, "body"),
        url: s(&v, "url"),
        additions: n(&v, "additions"),
        deletions: n(&v, "deletions"),
        changed_files: n(&v, "changedFiles"),
        created_at: s(&v, "createdAt"),
        updated_at: s(&v, "updatedAt"),
    })
}

/// Files of a `gh pr diff` output with Hydra's status words.
pub fn pr_files_from_diff(full_diff: &str) -> Vec<GitPrFile> {
    split_diff_by_file(full_diff)
        .into_iter()
        .map(|chunk| {
            let parsed = parse_unified_diff(&chunk.patch);
            let f = parsed.first();
            let status = match f.map(|f| f.status) {
                Some(DiffFileStatus::Added) => "added",
                Some(DiffFileStatus::Deleted) => "removed",
                Some(DiffFileStatus::Renamed) => "renamed",
                _ => "modified",
            };
            GitPrFile {
                path: chunk.path,
                status: status.into(),
                additions: f.map_or(0, |f| f.additions),
                deletions: f.map_or(0, |f| f.deletions),
                patch: chunk.patch,
            }
        })
        .collect()
}

impl GitRunner {
    /// Metadata and per-file patches of a PR (`number`, `#number` or URL).
    pub fn pr_fetch(&self, dir: &Path, number_or_url: &str) -> GitResult<GitPrDiff> {
        let id = parse_pr_identifier(number_or_url)?;
        let meta = self.gh(dir, &args!["pr", "view", id.as_str(), "--json", PR_FIELDS])?;
        let metadata = parse_pr_metadata(&meta)?;
        let diff = self.gh(dir, &args!["pr", "diff", id.as_str(), "--color=never"])?;
        Ok(GitPrDiff {
            metadata,
            files: pr_files_from_diff(&diff),
        })
    }

    /// Raw patch of one file of a PR (empty when the PR does not touch it).
    pub fn pr_file_diff(&self, dir: &Path, number_or_url: &str, path: &str) -> GitResult<String> {
        let id = parse_pr_identifier(number_or_url)?;
        let diff = self.gh(dir, &args!["pr", "diff", id.as_str(), "--color=never"])?;
        Ok(extract_file_diff(&diff, path))
    }
}
