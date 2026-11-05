//! Errors of the git/gh wrappers.

#[derive(Debug, thiserror::Error)]
pub enum GitError {
    #[error("git is not installed or not on PATH")]
    GitMissing,
    #[error("the GitHub CLI (gh) is not installed or not on PATH")]
    GhMissing,
    #[error("not a git repository: {0}")]
    NotARepo(String),
    #[error("{program} timed out after {secs} s")]
    Timeout { program: String, secs: u64 },
    #[error("{stderr}")]
    Failed { code: Option<i32>, stderr: String },
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error("could not parse git output: {0}")]
    Parse(String),
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
}

pub type GitResult<T> = Result<T, GitError>;
