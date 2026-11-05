//! Git panel: Changes (file list + colored unified diff, stage, commit, push), Branches
//! (checkout, new branch) and PR review (`gh`, read-only). Port of Hydra's `GitPanel`.

use as_core::DiffLayout;
use as_core::git::{GitBranch, GitDiffStats, GitPrDiff, GitStatus, GitStatusKind};
use as_proto::{Request, Response};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::diff_view::DiffView;
use super::{Outcome, Reply, is_char, nav, req, step};
use crate::app::Effect;
use crate::widgets::TextArea;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitTab {
    Changes,
    Branches,
    Pr,
}

impl GitTab {
    pub const ALL: [GitTab; 3] = [GitTab::Changes, GitTab::Branches, GitTab::Pr];

    pub fn label(self) -> &'static str {
        match self {
            GitTab::Changes => "Changes",
            GitTab::Branches => "Branches",
            GitTab::Pr => "PR review",
        }
    }

    fn index(self) -> usize {
        Self::ALL.iter().position(|t| *t == self).unwrap_or(0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptKind {
    Commit,
    NewBranch,
    PrId,
    ConfirmPush,
    /// Checkout of this branch while the work tree is dirty.
    ConfirmCheckout(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Prompt {
    pub kind: PromptKind,
    pub input: TextArea,
}

impl Prompt {
    fn new(kind: PromptKind) -> Self {
        Self {
            kind,
            input: TextArea::single_line(""),
        }
    }

    pub fn title(&self) -> String {
        match &self.kind {
            PromptKind::Commit => "Commit message (Enter commits the staged files)".into(),
            PromptKind::NewBranch => "New branch name".into(),
            PromptKind::PrId => "Pull request number or URL".into(),
            PromptKind::ConfirmPush => "Push the current branch? (y/n)".into(),
            PromptKind::ConfirmCheckout(b) => {
                format!("Uncommitted changes. Checkout {b} anyway? (y/n)")
            }
        }
    }

    pub fn is_confirm(&self) -> bool {
        matches!(
            self.kind,
            PromptKind::ConfirmPush | PromptKind::ConfirmCheckout(_)
        )
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct PrUi {
    pub id: String,
    pub data: Option<GitPrDiff>,
    pub cursor: usize,
    pub diff: DiffView,
    pub loading: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GitUi {
    pub tab: GitTab,
    /// The repository folder (the selected agent's work tree).
    pub dir: String,
    pub loaded: bool,
    pub loading: bool,
    /// `git`/`gh` missing, not a repository, or a failed command.
    pub error: Option<String>,
    pub status: GitStatus,
    pub stats: GitDiffStats,
    pub cursor: usize,
    pub diff: DiffView,
    pub diff_focus: bool,
    pub branches: Vec<GitBranch>,
    pub branch_cursor: usize,
    pub pr: PrUi,
    pub prompt: Option<Prompt>,
    /// Last result line: `(is_error, text)`.
    pub notice: Option<(bool, String)>,
    /// Rows the diff area shows (set by the renderer, used for paging).
    pub diff_height: usize,
    /// Requested diff layout (from the config; `s` toggles it).
    pub layout: DiffLayout,
    diff_seq: u64,
    /// What the shown diff belongs to, to skip reloads of the same file.
    diff_of: Option<(String, bool)>,
}

impl Default for GitUi {
    fn default() -> Self {
        Self {
            tab: GitTab::Changes,
            dir: String::new(),
            loaded: false,
            loading: false,
            error: None,
            status: GitStatus::default(),
            stats: GitDiffStats::default(),
            cursor: 0,
            diff: DiffView::default(),
            diff_focus: false,
            branches: Vec::new(),
            branch_cursor: 0,
            pr: PrUi::default(),
            prompt: None,
            notice: None,
            diff_height: 20,
            layout: DiffLayout::default(),
            diff_seq: 0,
            diff_of: None,
        }
    }
}

/// `MM`-style marker like `git status --short`.
pub fn marker(f: &as_core::git::GitFileStatus) -> String {
    if f.untracked {
        return "??".into();
    }
    let c = |k: Option<GitStatusKind>| k.map_or(' ', GitStatusKind::letter);
    format!("{}{}", c(f.index), c(f.worktree))
}

impl GitUi {
    /// Open (or re-open) the panel for a repository folder.
    pub fn open(&mut self, dir: &str) -> Vec<Effect> {
        if self.dir != dir {
            *self = Self {
                tab: self.tab,
                ..Self::default()
            };
            self.dir = dir.to_string();
        }
        self.prompt = None;
        self.refresh()
    }

    pub fn refresh(&mut self) -> Vec<Effect> {
        self.loading = true;
        let dir = self.dir.clone();
        let mut fx = vec![
            req("git:status", Request::GitStatus { dir: dir.clone() }),
            req("git:stats", Request::GitDiffStats { dir: dir.clone() }),
        ];
        if self.tab == GitTab::Branches {
            fx.push(req("git:branches", Request::GitBranches { dir }));
        }
        fx
    }

    pub fn selected_file(&self) -> Option<&as_core::git::GitFileStatus> {
        self.status.files.get(self.cursor)
    }

    fn load_diff(&mut self) -> Vec<Effect> {
        let Some(file) = self.selected_file() else {
            self.diff = DiffView::note("", "Nothing to show: the work tree is clean");
            self.diff_of = None;
            return Vec::new();
        };
        // Staged-only files show the index diff, everything else the work tree one.
        let staged = file.staged && !file.unstaged && !file.untracked;
        let path = file.path.clone();
        if self.diff_of.as_ref() == Some(&(path.clone(), staged)) {
            return Vec::new();
        }
        self.diff_of = Some((path.clone(), staged));
        self.diff_seq += 1;
        vec![req(
            format!("git:diff:{}", self.diff_seq),
            Request::GitDiff {
                dir: self.dir.clone(),
                file: Some(path),
                staged,
            },
        )]
    }

    pub fn on_reply(&mut self, key: &str, result: Reply) -> Vec<Effect> {
        self.loading = false;
        match (key, result) {
            ("git:status", Ok(Response::GitStatus(status))) => {
                self.error = None;
                self.loaded = true;
                if !status.is_repo {
                    self.error = Some("This folder is not a git repository".into());
                }
                let keep = self.selected_file().map(|f| f.path.clone());
                self.status = *status;
                self.cursor = keep
                    .and_then(|p| self.status.files.iter().position(|f| f.path == p))
                    .unwrap_or_else(|| self.cursor.min(self.status.files.len().saturating_sub(1)));
                self.diff_of = None;
                if self.error.is_none() && self.tab == GitTab::Changes {
                    return self.load_diff();
                }
                Vec::new()
            }
            ("git:status", Err(e)) => {
                self.loaded = true;
                self.error = Some(e);
                Vec::new()
            }
            ("git:stats", Ok(Response::GitDiffStats(s))) => {
                self.stats = s;
                Vec::new()
            }
            ("git:branches", Ok(Response::GitBranches(b))) => {
                self.branch_cursor = b
                    .iter()
                    .position(|x| x.is_current)
                    .unwrap_or(0)
                    .min(b.len().saturating_sub(1));
                self.branches = b;
                Vec::new()
            }
            ("git:branches", Err(e)) => {
                self.notice = Some((true, e));
                Vec::new()
            }
            ("git:pr", result) => {
                self.pr.loading = false;
                match result {
                    Ok(Response::GitPr(data)) => {
                        self.pr.error = None;
                        self.pr.cursor = 0;
                        self.pr.data = Some(*data);
                        self.show_pr_file();
                    }
                    Ok(_) => {}
                    Err(e) => {
                        self.pr.error = Some(e);
                        self.pr.data = None;
                    }
                }
                Vec::new()
            }
            (k, Ok(Response::Text(text))) if k.starts_with("git:diff:") => {
                if k == format!("git:diff:{}", self.diff_seq) {
                    let title = self
                        .selected_file()
                        .map(|f| f.path.clone())
                        .unwrap_or_default();
                    self.diff = DiffView::from_text(&title, &text, false).with_path(&title);
                    if text.trim().is_empty() {
                        self.diff =
                            DiffView::note(&title, "No textual changes (empty or binary file)");
                    }
                }
                Vec::new()
            }
            (k, Err(e)) if k.starts_with("git:diff:") => {
                self.diff = DiffView::note("", &e);
                Vec::new()
            }
            (k, Ok(res)) if k.starts_with("git:op:") => {
                let what = &k["git:op:".len()..];
                let line = match (what, res) {
                    ("commit", Response::Text(h)) => format!("Committed {h}"),
                    ("push", Response::Text(t)) => {
                        let t = t.trim();
                        if t.is_empty() {
                            "Pushed".to_string()
                        } else {
                            format!("Pushed: {}", t.lines().last().unwrap_or(""))
                        }
                    }
                    ("checkout", _) => "Switched branch".into(),
                    ("branch", _) => "Branch created".into(),
                    _ => String::new(),
                };
                if !line.is_empty() {
                    self.notice = Some((false, line));
                }
                self.diff_of = None;
                self.refresh_all()
            }
            (k, Err(e)) if k.starts_with("git:op:") => {
                self.notice = Some((true, e));
                self.refresh()
            }
            _ => Vec::new(),
        }
    }

    fn refresh_all(&mut self) -> Vec<Effect> {
        let mut fx = self.refresh();
        let dir = self.dir.clone();
        if self.tab != GitTab::Branches {
            fx.push(req("git:branches", Request::GitBranches { dir }));
        }
        fx
    }

    fn show_pr_file(&mut self) {
        let Some(data) = &self.pr.data else { return };
        self.pr.diff = match data.files.get(self.pr.cursor) {
            Some(f) => DiffView::from_text(&f.path, &f.patch, false).with_path(&f.path),
            None => DiffView::note("", "This pull request has no files"),
        };
    }

    fn set_tab(&mut self, tab: GitTab) -> Vec<Effect> {
        self.tab = tab;
        self.diff_focus = false;
        self.notice = None;
        match tab {
            GitTab::Branches => vec![req(
                "git:branches",
                Request::GitBranches {
                    dir: self.dir.clone(),
                },
            )],
            GitTab::Changes => self.load_diff(),
            GitTab::Pr => Vec::new(),
        }
    }

    pub fn key(&mut self, key: &KeyEvent) -> (Outcome, Vec<Effect>) {
        if self.prompt.is_some() {
            return (Outcome::Stay, self.prompt_key(key));
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc | KeyCode::Char('q')
                if !self.diff_focus || key.code == KeyCode::Char('q') =>
            {
                return (Outcome::Close, Vec::new());
            }
            KeyCode::Tab => {
                let t = GitTab::ALL[(self.tab.index() + 1) % 3];
                return (Outcome::Stay, self.set_tab(t));
            }
            KeyCode::BackTab => {
                let t = GitTab::ALL[(self.tab.index() + 2) % 3];
                return (Outcome::Stay, self.set_tab(t));
            }
            KeyCode::Char(c @ '1'..='3') if !ctrl => {
                let t = GitTab::ALL[usize::from(c as u8 - b'1')];
                return (Outcome::Stay, self.set_tab(t));
            }
            KeyCode::Char('r') if self.error.is_some() || !self.diff_focus => {
                return (Outcome::Stay, self.refresh());
            }
            KeyCode::Char('s') if !ctrl && self.tab != GitTab::Branches => {
                self.layout = self.layout.toggled();
                let value = serde_json::to_value(self.layout).unwrap_or_default();
                return (
                    Outcome::Stay,
                    vec![Effect::PatchConfig(
                        serde_json::json!({ "diffLayout": value }),
                    )],
                );
            }
            _ => {}
        }
        if self.error.is_some() && self.tab != GitTab::Pr {
            return (Outcome::Stay, Vec::new());
        }
        let fx = match self.tab {
            GitTab::Changes => self.changes_key(key),
            GitTab::Branches => self.branches_key(key),
            GitTab::Pr => self.pr_key(key),
        };
        (Outcome::Stay, fx)
    }

    fn changes_key(&mut self, key: &KeyEvent) -> Vec<Effect> {
        let page = self.diff_height.saturating_sub(1);
        if self.diff_focus {
            match key.code {
                KeyCode::Esc | KeyCode::Left | KeyCode::Char('h') => self.diff_focus = false,
                KeyCode::Char('n') => self.diff.next_hunk(self.diff_height),
                KeyCode::Char('N') => self.diff.prev_hunk(),
                _ => {
                    if let Some(d) = nav(key, page) {
                        self.diff.scroll_by(d, self.diff_height);
                    }
                }
            }
            return Vec::new();
        }
        match key.code {
            KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => {
                self.diff_focus = true;
                return Vec::new();
            }
            KeyCode::Char(' ') => return self.toggle_stage(),
            KeyCode::Char('a') => {
                let files: Vec<String> = self.status.files.iter().map(|f| f.path.clone()).collect();
                if files.is_empty() {
                    return Vec::new();
                }
                return vec![req(
                    "git:op:stage",
                    Request::GitStage {
                        dir: self.dir.clone(),
                        files,
                    },
                )];
            }
            KeyCode::Char('c') => {
                if self.status.files.iter().any(|f| f.staged) {
                    self.prompt = Some(Prompt::new(PromptKind::Commit));
                } else {
                    self.notice =
                        Some((true, "Nothing staged: press space to stage a file".into()));
                }
                return Vec::new();
            }
            KeyCode::Char('P') => {
                self.prompt = Some(Prompt::new(PromptKind::ConfirmPush));
                return Vec::new();
            }
            KeyCode::Char('n') => self.diff.next_hunk(self.diff_height),
            KeyCode::Char('N') => self.diff.prev_hunk(),
            _ => {
                if let Some(d) = nav(key, page) {
                    self.cursor = step(self.cursor, self.status.files.len(), d);
                    return self.load_diff();
                }
            }
        }
        Vec::new()
    }

    fn toggle_stage(&mut self) -> Vec<Effect> {
        let Some(f) = self.selected_file() else {
            return Vec::new();
        };
        let (dir, files) = (self.dir.clone(), vec![f.path.clone()]);
        // A file with unstaged edits (or untracked) gets staged; a fully staged one is unstaged.
        let request = if f.staged && !f.unstaged && !f.untracked {
            Request::GitUnstage { dir, files }
        } else {
            Request::GitStage { dir, files }
        };
        vec![req("git:op:stage", request)]
    }

    fn branches_key(&mut self, key: &KeyEvent) -> Vec<Effect> {
        match key.code {
            KeyCode::Char('n') => {
                self.prompt = Some(Prompt::new(PromptKind::NewBranch));
            }
            KeyCode::Enter => {
                let Some(b) = self.branches.get(self.branch_cursor) else {
                    return Vec::new();
                };
                if b.is_current {
                    return Vec::new();
                }
                let name = b.name.clone();
                if self.status.files.is_empty() {
                    return self.checkout(&name);
                }
                self.prompt = Some(Prompt::new(PromptKind::ConfirmCheckout(name)));
            }
            _ => {
                if let Some(d) = nav(key, 10) {
                    self.branch_cursor = step(self.branch_cursor, self.branches.len(), d);
                }
            }
        }
        Vec::new()
    }

    fn checkout(&mut self, branch: &str) -> Vec<Effect> {
        vec![req(
            "git:op:checkout",
            Request::GitCheckout {
                dir: self.dir.clone(),
                branch: branch.to_string(),
            },
        )]
    }

    fn pr_key(&mut self, key: &KeyEvent) -> Vec<Effect> {
        if self.diff_focus {
            let page = self.diff_height.saturating_sub(1);
            match key.code {
                KeyCode::Esc | KeyCode::Left | KeyCode::Char('h') => self.diff_focus = false,
                KeyCode::Char('n') => self.pr.diff.next_hunk(self.diff_height),
                KeyCode::Char('N') => self.pr.diff.prev_hunk(),
                _ => {
                    if let Some(d) = nav(key, page) {
                        self.pr.diff.scroll_by(d, self.diff_height);
                    }
                }
            }
            return Vec::new();
        }
        match key.code {
            KeyCode::Char('/' | 'p') => self.prompt = Some(Prompt::new(PromptKind::PrId)),
            KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => {
                if self.pr.data.is_some() {
                    self.diff_focus = true;
                } else {
                    self.prompt = Some(Prompt::new(PromptKind::PrId));
                }
            }
            _ => {
                if let (Some(d), Some(data)) = (nav(key, 10), &self.pr.data) {
                    self.pr.cursor = step(self.pr.cursor, data.files.len(), d);
                    self.show_pr_file();
                }
            }
        }
        Vec::new()
    }

    fn prompt_key(&mut self, key: &KeyEvent) -> Vec<Effect> {
        let Some(mut prompt) = self.prompt.take() else {
            return Vec::new();
        };
        if key.code == KeyCode::Esc {
            return Vec::new();
        }
        if prompt.is_confirm() {
            if is_char(key, 'y') || key.code == KeyCode::Enter {
                return match prompt.kind {
                    PromptKind::ConfirmPush => vec![req(
                        "git:op:push",
                        Request::GitPush {
                            dir: self.dir.clone(),
                        },
                    )],
                    PromptKind::ConfirmCheckout(b) => self.checkout(&b),
                    _ => Vec::new(),
                };
            }
            if is_char(key, 'n') {
                return Vec::new();
            }
            self.prompt = Some(prompt);
            return Vec::new();
        }
        if key.code == KeyCode::Enter {
            let text = prompt.input.text().trim().to_string();
            if text.is_empty() {
                return Vec::new();
            }
            return match prompt.kind {
                PromptKind::Commit => vec![req(
                    "git:op:commit",
                    Request::GitCommit {
                        dir: self.dir.clone(),
                        message: text,
                        files: Vec::new(),
                    },
                )],
                PromptKind::NewBranch => vec![req(
                    "git:op:branch",
                    Request::GitCreateBranch {
                        dir: self.dir.clone(),
                        name: text,
                        from: None,
                    },
                )],
                PromptKind::PrId => {
                    self.pr.id.clone_from(&text);
                    self.pr.loading = true;
                    self.pr.error = None;
                    self.pr.data = None;
                    vec![req(
                        "git:pr",
                        Request::GitPrFetch {
                            dir: self.dir.clone(),
                            pr: text,
                        },
                    )]
                }
                _ => Vec::new(),
            };
        }
        prompt.input.handle_key(key);
        self.prompt = Some(prompt);
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use as_core::git::GitFileStatus;
    use ratatui::crossterm::event::KeyEvent;

    fn k(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    fn file(
        path: &str,
        index: Option<GitStatusKind>,
        worktree: Option<GitStatusKind>,
    ) -> GitFileStatus {
        GitFileStatus {
            path: path.into(),
            orig_path: None,
            kind: index.or(worktree).unwrap_or(GitStatusKind::Untracked),
            index,
            worktree,
            staged: index.is_some(),
            unstaged: worktree.is_some(),
            untracked: false,
            conflicted: false,
        }
    }

    fn ui_with(files: Vec<GitFileStatus>) -> GitUi {
        GitUi {
            dir: "/repo".into(),
            status: GitStatus {
                is_repo: true,
                branch: "main".into(),
                files,
                ..Default::default()
            },
            loaded: true,
            ..GitUi::default()
        }
    }

    #[test]
    fn markers_follow_porcelain() {
        let mut f = file(
            "a",
            Some(GitStatusKind::Modified),
            Some(GitStatusKind::Modified),
        );
        assert_eq!(marker(&f), "MM");
        f.worktree = None;
        assert_eq!(marker(&f), "M ");
        let mut u = file("b", None, None);
        u.untracked = true;
        assert_eq!(marker(&u), "??");
    }

    #[test]
    fn space_stages_unstaged_and_unstages_staged() {
        let mut ui = ui_with(vec![
            file("a", None, Some(GitStatusKind::Modified)),
            file("b", Some(GitStatusKind::Added), None),
        ]);
        let (_, fx) = ui.key(&k(' '));
        assert!(
            matches!(&fx[..], [Effect::Req { req: Request::GitStage { files, .. }, .. }] if files == &["a"])
        );
        ui.cursor = 1;
        let (_, fx) = ui.key(&k(' '));
        assert!(
            matches!(&fx[..], [Effect::Req { req: Request::GitUnstage { files, .. }, .. }] if files == &["b"])
        );
    }

    #[test]
    fn commit_needs_staged_files_then_asks_for_a_message() {
        let mut ui = ui_with(vec![file("a", None, Some(GitStatusKind::Modified))]);
        ui.key(&k('c'));
        assert!(ui.prompt.is_none());
        assert!(ui.notice.as_ref().is_some_and(|n| n.0));
        ui.status.files[0].staged = true;
        ui.key(&k('c'));
        assert!(ui.prompt.is_some());
        for c in "fix x".chars() {
            ui.key(&k(c));
        }
        let (_, fx) = ui.key(&KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(
            matches!(&fx[..], [Effect::Req { key, req: Request::GitCommit { message, .. } }] if key == "git:op:commit" && message == "fix x")
        );
        assert!(ui.prompt.is_none());
    }

    #[test]
    fn push_asks_for_confirmation() {
        let mut ui = ui_with(Vec::new());
        ui.key(&k('P'));
        assert!(ui.prompt.as_ref().is_some_and(Prompt::is_confirm));
        let (_, fx) = ui.key(&k('n'));
        assert!(fx.is_empty() && ui.prompt.is_none());
        ui.key(&k('P'));
        let (_, fx) = ui.key(&k('y'));
        assert!(matches!(
            &fx[..],
            [Effect::Req {
                req: Request::GitPush { .. },
                ..
            }]
        ));
    }

    #[test]
    fn checkout_confirms_when_dirty() {
        let branch = |n: &str, cur: bool| GitBranch {
            name: n.into(),
            is_current: cur,
            is_remote: false,
            upstream: None,
            ahead_of_upstream: 0,
            behind_upstream: 0,
        };
        let mut ui = ui_with(vec![file("a", None, Some(GitStatusKind::Modified))]);
        ui.tab = GitTab::Branches;
        ui.branches = vec![branch("main", true), branch("dev", false)];
        ui.branch_cursor = 1;
        let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        let (_, fx) = ui.key(&enter);
        assert!(fx.is_empty());
        assert!(
            matches!(ui.prompt.as_ref().map(|p| &p.kind), Some(PromptKind::ConfirmCheckout(b)) if b == "dev")
        );
        let (_, fx) = ui.key(&k('y'));
        assert!(
            matches!(&fx[..], [Effect::Req { req: Request::GitCheckout { branch, .. }, .. }] if branch == "dev")
        );
        // Clean tree: no confirmation.
        ui.status.files.clear();
        let (_, fx) = ui.key(&enter);
        assert_eq!(fx.len(), 1);
        // The current branch does nothing.
        ui.branch_cursor = 0;
        let (_, fx) = ui.key(&enter);
        assert!(fx.is_empty());
    }

    #[test]
    fn stale_diff_replies_are_ignored() {
        let mut ui = ui_with(vec![
            file("a", None, Some(GitStatusKind::Modified)),
            file("b", None, Some(GitStatusKind::Modified)),
        ]);
        let first = ui.load_diff();
        let first_key = match &first[0] {
            Effect::Req { key, .. } => key.clone(),
            _ => unreachable!(),
        };
        ui.cursor = 1;
        ui.load_diff();
        let text = "@@ -1 +1 @@\n-x\n+y\n".to_string();
        ui.on_reply(&first_key, Ok(Response::Text(text.clone())));
        assert!(ui.diff.rows.is_empty(), "the old answer is dropped");
        let current = format!("git:diff:{}", ui.diff_seq);
        ui.on_reply(&current, Ok(Response::Text(text)));
        assert_eq!(ui.diff.hunks.len(), 1);
    }

    #[test]
    fn tabs_switch_with_tab_and_digits_and_pr_fetch_sends_the_id() {
        let mut ui = ui_with(Vec::new());
        ui.key(&KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(ui.tab, GitTab::Branches);
        ui.key(&k('3'));
        assert_eq!(ui.tab, GitTab::Pr);
        ui.key(&k('/'));
        for c in "42".chars() {
            ui.key(&k(c));
        }
        let (_, fx) = ui.key(&KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(
            matches!(&fx[..], [Effect::Req { req: Request::GitPrFetch { pr, .. }, .. }] if pr == "42")
        );
        assert!(ui.pr.loading);
        let (out, _) = ui.key(&k('q'));
        assert_eq!(out, Outcome::Close);
    }

    #[test]
    fn not_a_repo_is_reported() {
        let mut ui = GitUi::default();
        ui.on_reply("git:status", Ok(Response::GitStatus(Box::default())));
        assert!(
            ui.error
                .as_deref()
                .is_some_and(|e| e.contains("not a git repository"))
        );
        let (_, fx) = ui.key(&k(' '));
        assert!(fx.is_empty());
        ui.on_reply(
            "git:status",
            Err("git is not installed or not on PATH".into()),
        );
        assert!(
            ui.error
                .as_deref()
                .is_some_and(|e| e.contains("not installed"))
        );
    }
}
