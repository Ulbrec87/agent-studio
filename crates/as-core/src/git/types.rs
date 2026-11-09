//! Wire types of the git panel (Hydra `GitStatus`, `GitCommit`, `GitBranch`, ...).

use serde::{Deserialize, Serialize};

/// Kind of change of a file (`git status` letter).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
pub enum GitStatusKind {
    #[serde(rename = "M")]
    Modified,
    #[serde(rename = "A")]
    Added,
    #[serde(rename = "D")]
    Deleted,
    #[serde(rename = "R")]
    Renamed,
    #[serde(rename = "C")]
    Copied,
    #[serde(rename = "T")]
    TypeChanged,
    #[serde(rename = "?")]
    Untracked,
    /// Unmerged (conflict).
    #[serde(rename = "U")]
    Unmerged,
}

impl GitStatusKind {
    pub fn letter(self) -> char {
        match self {
            Self::Modified => 'M',
            Self::Added => 'A',
            Self::Deleted => 'D',
            Self::Renamed => 'R',
            Self::Copied => 'C',
            Self::TypeChanged => 'T',
            Self::Untracked => '?',
            Self::Unmerged => 'U',
        }
    }
}

/// One entry of `git status`. `path` is relative to the repository root and uses `/`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct GitFileStatus {
    pub path: String,
    /// Old path of a rename/copy.
    pub orig_path: Option<String>,
    /// Primary kind: `?` when untracked, `U` when conflicted, else the staged kind, else the unstaged one.
    pub kind: GitStatusKind,
    /// Change recorded in the index (staged), if any.
    pub index: Option<GitStatusKind>,
    /// Change in the work tree not yet staged, if any.
    pub worktree: Option<GitStatusKind>,
    pub staged: bool,
    pub unstaged: bool,
    pub untracked: bool,
    pub conflicted: bool,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct GitStatus {
    pub is_repo: bool,
    /// Current branch; empty when HEAD is detached or the folder is not a repository.
    pub branch: String,
    pub detached: bool,
    /// Abbreviated HEAD commit (`None` on an unborn branch).
    pub head: Option<String>,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub files: Vec<GitFileStatus>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct GitCommit {
    pub hash: String,
    /// First 8 characters (what Hydra shows).
    pub short_hash: String,
    pub message: String,
    pub author: String,
    /// Author date, strict ISO 8601.
    pub date: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct GitDiffStats {
    pub files: u32,
    pub additions: u32,
    pub deletions: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct GitBranch {
    /// Short name (`main`, `origin/main`).
    pub name: String,
    pub is_current: bool,
    pub is_remote: bool,
    pub upstream: Option<String>,
    pub ahead_of_upstream: u32,
    pub behind_upstream: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct GitFileContents {
    pub original: String,
    pub modified: String,
    pub language: String,
    /// One of the sides contains NUL bytes.
    pub binary: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct GitPrMetadata {
    pub number: u64,
    pub title: String,
    pub author: String,
    pub state: String,
    pub base_ref: String,
    pub head_ref: String,
    pub body: String,
    pub url: String,
    pub additions: u64,
    pub deletions: u64,
    pub changed_files: u64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct GitPrFile {
    pub path: String,
    /// `added` | `removed` | `renamed` | `modified` (Hydra's words).
    pub status: String,
    pub additions: u32,
    pub deletions: u32,
    /// Raw unified diff of the file.
    pub patch: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct GitPrDiff {
    pub metadata: GitPrMetadata,
    pub files: Vec<GitPrFile>,
}

/// A worktree of a repository (`git worktree list`), or one just created.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct GitWorktree {
    pub path: String,
    pub branch: Option<String>,
    pub head: Option<String>,
    pub bare: bool,
    pub detached: bool,
    pub locked: bool,
    pub prunable: bool,
}

/// Language id used by diff/preview views, from the file extension (Hydra `LANG_MAP`).
pub fn detect_language(path: &str) -> &'static str {
    let ext = path
        .rsplit('.')
        .next()
        .filter(|_| path.contains('.'))
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "ts" | "tsx" => "typescript",
        "js" | "jsx" => "javascript",
        "json" => "json",
        "css" => "css",
        "scss" => "scss",
        "html" => "html",
        "md" => "markdown",
        "py" => "python",
        "rs" => "rust",
        "go" => "go",
        "yaml" | "yml" => "yaml",
        "sh" | "bash" | "zsh" => "shell",
        "sql" => "sql",
        "xml" | "svg" => "xml",
        "toml" => "ini",
        "rb" => "ruby",
        "java" => "java",
        "kt" => "kotlin",
        "swift" => "swift",
        "c" | "h" => "c",
        "cpp" | "hpp" => "cpp",
        _ => "plaintext",
    }
}
