//! State and logic of the "New agent" dialog (Hydra `NewAgentDialog.tsx`), including resuming a
//! session (picker popup) and the Claude profile of the project.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use as_core::paths::{expand_tilde, tildify};
use as_core::sessions::profiles::{config_dir_for_transcript, resolve_profile};
use as_core::sessions::{ClaudeProfile, SessionSummary};
use as_core::{
    AgentManager, AppConfig, CODEX_REASONING_LEVELS, CreateAgentPayload, ModelId, ProviderId,
    ProviderModelOption, get_default_model_for_provider, is_valid_model_for_provider,
    provider_models,
};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::sessions::{PickerAction, PickerOrigin, SessionPicker};
use crate::widgets::TextArea;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Name,
    ProjectDir,
    Provider,
    Model,
    Effort,
    /// Claude profile of the project (only with more than one profile).
    Profile,
    Yolo,
    /// Resume a previous session (opens the session picker).
    Resume,
    Prompt,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FormAction {
    None,
    Cancel,
    Submit,
    /// The session picker inside the dialog wants something done (load, hide).
    Picker(PickerAction),
}

#[derive(Debug, Clone, PartialEq)]
pub struct NewAgentForm {
    pub field: Field,
    pub name: TextArea,
    pub dir: TextArea,
    pub provider: ProviderId,
    pub model_text: TextArea,
    /// Index into `provider_models(provider)` while a catalog model is selected; `None` = free text.
    pub model_idx: Option<usize>,
    /// 0 = provider default, otherwise `CODEX_REASONING_LEVELS[effort_idx - 1]`.
    pub effort_idx: usize,
    pub yolo: bool,
    pub prompt: TextArea,
    pub error: Option<String>,
    pub submitting: bool,
    /// Directory candidates of the last Tab completion (shown under the field).
    pub completions: Vec<String>,
    /// Models offered for `provider` (the daemon's discovered list, the built-in one until then).
    models: Vec<ProviderModelOption>,
    models_provider: ProviderId,
    /// Set when the provider changed: the app asks the daemon for its models.
    pub models_wanted: Option<ProviderId>,
    /// Claude profiles on this machine, and the project -> profile mapping of the config.
    pub profiles: Vec<ClaudeProfile>,
    profile_by_project: BTreeMap<String, String>,
    /// Index into `profiles`.
    pub profile_idx: usize,
    profile_touched: bool,
    /// The session to resume.
    pub resume: Option<SessionSummary>,
    pub picker: Option<Box<SessionPicker>>,
    session_max_age_days: u32,
    /// The configured default model, preferred whenever the provider offers it.
    preferred: String,
    cwd: PathBuf,
}

fn default_model_index(models: &[ProviderModelOption], preferred: &str) -> usize {
    models
        .iter()
        .position(|m| m.id == preferred)
        .or_else(|| models.iter().position(|m| m.is_default))
        .unwrap_or(0)
}

impl NewAgentForm {
    /// `default_dir` is the selected agent's project, the config's default or the cwd.
    pub fn new(config: &AppConfig, default_dir: &str, cwd: &Path) -> Self {
        let provider = config.default_provider;
        let models = provider_models(provider);
        let idx = default_model_index(&models, &config.default_model);
        let mut dir = TextArea::single_line(default_dir);
        dir.move_end();
        Self {
            field: Field::Name,
            name: TextArea::single_line(""),
            dir,
            provider,
            model_text: TextArea::single_line(""),
            model_idx: Some(idx),
            effort_idx: 0,
            yolo: config.global_yolo,
            prompt: TextArea::multi_line(),
            error: None,
            submitting: false,
            completions: Vec::new(),
            models,
            models_provider: provider,
            models_wanted: Some(provider),
            profiles: Vec::new(),
            profile_by_project: config.claude_profile_by_project.clone(),
            profile_idx: 0,
            profile_touched: false,
            resume: None,
            picker: None,
            session_max_age_days: config.session_max_age_days,
            preferred: config.default_model.clone(),
            cwd: cwd.to_path_buf(),
        }
    }

    /// The daemon's model list for `provider` (ignored when the provider changed meanwhile).
    /// Keeps the selected model when it is still offered.
    pub fn set_models(&mut self, provider: ProviderId, models: Vec<ProviderModelOption>) {
        if provider != self.provider || models.is_empty() {
            return;
        }
        let current = self
            .model_idx
            .and_then(|i| self.models.get(i))
            .map(|m| m.id.clone());
        let wanted = current.unwrap_or_else(|| self.preferred.clone());
        self.model_idx = self
            .model_idx
            .map(|_| default_model_index(&models, &wanted));
        self.models = models;
        self.models_provider = provider;
    }

    /// The Claude profiles of this machine; selects the one mapped to the project.
    pub fn set_profiles(&mut self, profiles: Vec<ClaudeProfile>) {
        self.profiles = profiles;
        self.profile_touched = false;
        self.default_profile();
    }

    /// Select the profile the project is mapped to (or the default one).
    fn default_profile(&mut self) {
        if self.profile_touched || self.profiles.is_empty() {
            return;
        }
        let dir = self.project_dir_text();
        let mapped = resolve_profile(&dir, &self.profile_by_project);
        self.profile_idx = self.profile_position(mapped.as_deref().unwrap_or(""));
    }

    /// Position of a mapped config dir (`""` = default) in `profiles`; the default when unknown.
    fn profile_position(&self, mapped: &str) -> usize {
        self.profiles
            .iter()
            .position(|p| {
                if mapped.is_empty() {
                    p.is_default
                } else {
                    p.config_dir == mapped
                }
            })
            .or_else(|| self.profiles.iter().position(|p| p.is_default))
            .unwrap_or(0)
    }

    fn project_dir_text(&self) -> String {
        let text = self.dir.text();
        let mut dir = expand_tilde(text.trim());
        if dir.is_relative() {
            dir = self.cwd.join(dir);
        }
        dir.to_string_lossy()
            .trim_end_matches(['/', '\\'])
            .to_string()
    }

    /// Does the dialog offer a profile choice?
    pub fn shows_profile(&self) -> bool {
        self.provider == ProviderId::Claude && self.profiles.len() > 1
    }

    /// The profile of the session being resumed (a resumed session lives in the profile that saved it).
    pub fn resume_profile(&self) -> Option<&ClaudeProfile> {
        let session = self.resume.as_ref()?;
        let dir =
            config_dir_for_transcript(Path::new(&session.source_path), &as_core::paths::home_dir());
        self.profiles.iter().find(|p| {
            if dir.is_empty() {
                p.is_default
            } else {
                p.config_dir == dir
            }
        })
    }

    /// `(project_dir, config_dir)` to map before creating the agent, when the user picked a
    /// profile other than the project's current one. `""` = the default profile.
    pub fn profile_change(&self, project_dir: &str) -> Option<(String, String)> {
        if !self.shows_profile() || self.resume.is_some() {
            return None;
        }
        let chosen = self.profiles.get(self.profile_idx)?;
        let chosen_dir = if chosen.is_default {
            ""
        } else {
            chosen.config_dir.as_str()
        };
        let current = resolve_profile(project_dir, &self.profile_by_project).unwrap_or_default();
        (chosen_dir != current).then(|| (project_dir.to_string(), chosen_dir.to_string()))
    }

    /// Focusable fields in order.
    pub fn fields(&self) -> Vec<Field> {
        let mut f = vec![
            Field::Name,
            Field::ProjectDir,
            Field::Provider,
            Field::Model,
        ];
        if self.provider == ProviderId::Codex {
            f.push(Field::Effort);
        }
        if self.shows_profile() && self.resume.is_none() {
            f.push(Field::Profile);
        }
        f.push(Field::Yolo);
        f.push(Field::Resume);
        f.push(Field::Prompt);
        f
    }

    pub fn model_options(&self) -> Vec<ProviderModelOption> {
        if self.models_provider == self.provider {
            self.models.clone()
        } else {
            provider_models(self.provider)
        }
    }

    /// The model that would be created.
    pub fn model(&self) -> ModelId {
        if let Some(i) = self.model_idx
            && let Some(m) = self.model_options().get(i)
        {
            return m.id.clone();
        }
        let text = self.model_text.text();
        let text = text.trim();
        if text.is_empty() {
            get_default_model_for_provider(self.provider)
        } else {
            text.to_string()
        }
    }

    pub fn effort(&self) -> Option<&'static str> {
        (self.provider == ProviderId::Codex && self.effort_idx > 0)
            .then(|| CODEX_REASONING_LEVELS[self.effort_idx - 1])
    }

    fn move_field(&mut self, delta: isize) {
        let fields = self.fields();
        let pos = fields.iter().position(|f| *f == self.field).unwrap_or(0);
        let next = (pos as isize + delta).rem_euclid(fields.len() as isize) as usize;
        let left = self.field;
        self.field = fields[next];
        self.completions.clear();
        if left == Field::ProjectDir && self.field != Field::ProjectDir {
            self.default_profile();
        }
    }

    fn set_provider(&mut self, provider: ProviderId) {
        if provider == self.provider {
            return;
        }
        self.provider = provider;
        self.models = provider_models(provider);
        self.models_provider = provider;
        self.models_wanted = Some(provider);
        self.model_idx = Some(default_model_index(&self.model_options(), &self.preferred));
        self.model_text.clear();
        self.effort_idx = 0;
        // A session belongs to the provider that saved it.
        if self.resume.as_ref().is_some_and(|s| s.provider != provider) {
            self.resume = None;
        }
    }

    fn cycle_provider(&mut self, delta: isize) {
        let all = ProviderId::ALL;
        let pos = all.iter().position(|p| *p == self.provider).unwrap_or(0);
        let next = (pos as isize + delta).rem_euclid(all.len() as isize) as usize;
        self.set_provider(all[next]);
    }

    fn cycle_model(&mut self, delta: isize) {
        let n = self.model_options().len() as isize;
        let next = match self.model_idx {
            Some(i) => (i as isize + delta).rem_euclid(n),
            None => {
                // From free text: jump to the matching catalog entry or the edge.
                let text = self.model_text.text();
                self.model_options()
                    .iter()
                    .position(|m| m.id == text.trim())
                    .map_or(if delta > 0 { 0 } else { n - 1 }, |i| i as isize)
            }
        };
        self.model_idx = Some(next as usize);
        self.model_text.clear();
    }

    fn cycle_effort(&mut self, delta: isize) {
        let n = CODEX_REASONING_LEVELS.len() as isize + 1;
        self.effort_idx = (self.effort_idx as isize + delta).rem_euclid(n) as usize;
    }

    /// Tab in the directory field. Returns whether it changed anything.
    fn complete_dir(&mut self) -> bool {
        let text = self.dir.text();
        let (changed, candidates) = complete_path(&text, &self.cwd);
        self.completions = candidates;
        match changed {
            Some(new_text) if new_text != text => {
                self.dir.set_text(&new_text);
                true
            }
            _ => !self.completions.is_empty(),
        }
    }

    pub fn handle_key(&mut self, key: &KeyEvent) -> FormAction {
        if key.kind == KeyEventKind::Release || self.submitting {
            return FormAction::None;
        }
        if self.picker.is_some() {
            return self.picker_key(key);
        }
        let mods = key.modifiers;
        let ctrl = mods.contains(KeyModifiers::CONTROL);
        let alt = mods.contains(KeyModifiers::ALT) || mods.contains(KeyModifiers::SHIFT);
        self.error = None;
        match key.code {
            KeyCode::Esc => return FormAction::Cancel,
            KeyCode::Char('s') if ctrl => return FormAction::Submit,
            KeyCode::Enter if self.field == Field::Prompt && alt => {
                self.prompt.insert_newline();
                return FormAction::None;
            }
            KeyCode::Enter => return FormAction::Submit,
            KeyCode::BackTab => {
                self.move_field(-1);
                return FormAction::None;
            }
            KeyCode::Tab => {
                if self.field == Field::ProjectDir && self.complete_dir() {
                    return FormAction::None;
                }
                self.move_field(1);
                return FormAction::None;
            }
            _ => {}
        }
        match self.field {
            Field::Name => {
                if !self.name.handle_key(key) {
                    self.nav(key);
                }
            }
            Field::ProjectDir => {
                self.completions.clear();
                if !self.dir.handle_key(key) {
                    self.nav(key);
                }
            }
            Field::Provider => match key.code {
                KeyCode::Left | KeyCode::Char('h') => self.cycle_provider(-1),
                KeyCode::Right | KeyCode::Char('l') | KeyCode::Char(' ') => self.cycle_provider(1),
                _ => self.nav(key),
            },
            Field::Model => self.model_key(key),
            Field::Effort => match key.code {
                KeyCode::Left | KeyCode::Char('h') => self.cycle_effort(-1),
                KeyCode::Right | KeyCode::Char('l') | KeyCode::Char(' ') => self.cycle_effort(1),
                _ => self.nav(key),
            },
            Field::Profile => match key.code {
                KeyCode::Left | KeyCode::Char('h') => self.cycle_profile(-1),
                KeyCode::Right | KeyCode::Char('l') | KeyCode::Char(' ') => self.cycle_profile(1),
                _ => self.nav(key),
            },
            Field::Yolo => match key.code {
                KeyCode::Char(' ') | KeyCode::Left | KeyCode::Right | KeyCode::Char('y') => {
                    self.yolo = !self.yolo;
                }
                _ => self.nav(key),
            },
            Field::Resume => match key.code {
                KeyCode::Char(' ' | 'o') | KeyCode::Right => return self.open_picker(),
                KeyCode::Backspace | KeyCode::Delete | KeyCode::Left => self.resume = None,
                _ => self.nav(key),
            },
            Field::Prompt => {
                if !self.prompt.handle_key(key) && key.code == KeyCode::Up {
                    self.move_field(-1);
                }
            }
        }
        FormAction::None
    }

    fn cycle_profile(&mut self, delta: isize) {
        let n = self.profiles.len() as isize;
        if n > 0 {
            self.profile_idx = (self.profile_idx as isize + delta).rem_euclid(n) as usize;
            self.profile_touched = true;
        }
    }

    /// Open the session picker for the dialog's provider and directory.
    fn open_picker(&mut self) -> FormAction {
        let picker = SessionPicker::new(
            PickerOrigin::Form,
            self.provider,
            &self.project_dir_text(),
            self.session_max_age_days,
        );
        self.picker = Some(Box::new(picker));
        FormAction::Picker(PickerAction::Load {
            provider: self.provider,
            force: false,
        })
    }

    fn picker_key(&mut self, key: &KeyEvent) -> FormAction {
        let Some(picker) = self.picker.as_mut() else {
            return FormAction::None;
        };
        match picker.handle_key(key) {
            PickerAction::None => FormAction::None,
            PickerAction::Cancel => {
                self.picker = None;
                FormAction::None
            }
            PickerAction::Pick(session) => {
                self.picker = None;
                self.apply_session(*session);
                FormAction::None
            }
            other => FormAction::Picker(other),
        }
    }

    /// A session was picked: its project and provider fill the dialog.
    pub fn apply_session(&mut self, session: SessionSummary) {
        if !session.project_path.is_empty() {
            self.dir
                .set_text(&tildify(Path::new(&session.project_path)));
        }
        self.set_provider(session.provider);
        if self.name.is_empty() {
            self.name.set_text(&AgentManager::generate_name(
                &session.first_prompt,
                &session.project_path,
            ));
        }
        self.resume = Some(session);
    }

    /// Up/Down move between fields where the field itself has no use for them.
    fn nav(&mut self, key: &KeyEvent) {
        match key.code {
            KeyCode::Down => self.move_field(1),
            KeyCode::Up => self.move_field(-1),
            _ => {}
        }
    }

    fn model_key(&mut self, key: &KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('k') if ctrl => {
                // Back to the catalog list.
                self.model_idx = Some(default_model_index(&self.model_options(), &self.preferred));
                self.model_text.clear();
            }
            KeyCode::Left if self.model_idx.is_some() || self.model_text.is_empty() => {
                self.cycle_model(-1);
            }
            KeyCode::Right if self.model_idx.is_some() || self.model_text.is_empty() => {
                self.cycle_model(1);
            }
            KeyCode::Char(c) if !ctrl && !key.modifiers.contains(KeyModifiers::ALT) => {
                // Typing switches to free text.
                if self.model_idx.take().is_some() {
                    self.model_text.clear();
                }
                self.model_text.insert_char(c);
            }
            KeyCode::Backspace if self.model_idx.is_some() => {
                self.model_idx = None;
                self.model_text.clear();
            }
            _ => {
                if self.model_idx.is_none() && self.model_text.handle_key(key) {
                    return;
                }
                self.nav(key);
            }
        }
    }

    pub fn handle_paste(&mut self, text: &str) {
        match self.field {
            Field::Name => self.name.insert_str(text),
            Field::ProjectDir => self.dir.insert_str(text.trim()),
            Field::Model => {
                if self.model_idx.take().is_some() {
                    self.model_text.clear();
                }
                self.model_text.insert_str(text.trim());
            }
            Field::Prompt => self.prompt.insert_str(text),
            _ => {}
        }
    }

    pub fn handle_picker_paste(&mut self, text: &str) -> bool {
        match self.picker.as_mut() {
            Some(p) => {
                p.handle_paste(text);
                true
            }
            None => false,
        }
    }

    /// Validate and build the creation request.
    pub fn build_payload(&self) -> Result<CreateAgentPayload, String> {
        let dir_text = self.dir.text();
        let dir_text = dir_text.trim();
        if dir_text.is_empty() {
            return Err("Project directory is required".into());
        }
        let mut dir = expand_tilde(dir_text);
        if dir.is_relative() {
            dir = self.cwd.join(dir);
        }
        if !dir.is_dir() {
            return Err(format!("Directory does not exist: {}", tildify(&dir)));
        }
        let model = self.model();
        if self.model_idx.is_none()
            && !is_valid_model_for_provider(self.provider, &model)
            && model.chars().any(char::is_whitespace)
        {
            return Err("Model ids cannot contain spaces".into());
        }
        Ok(CreateAgentPayload {
            resume_session_id: self.resume.as_ref().map(|s| s.session_id.clone()),
            name: self.name.text().trim().to_string(),
            project_dir: dir
                .to_string_lossy()
                .trim_end_matches(['/', '\\'])
                .to_string(),
            provider: self.provider,
            model,
            reasoning_effort: self.effort().map(str::to_string),
            yolo: self.yolo,
            initial_prompt: self.prompt.text().trim().to_string(),
            ..Default::default()
        })
    }
}

/// Complete the last path component of `text` against directories.
/// Returns the new text (when unambiguous progress is possible) and the candidates (max 8 names).
pub fn complete_path(text: &str, cwd: &Path) -> (Option<String>, Vec<String>) {
    let (dir_part, prefix) = match text.rfind(['/', std::path::MAIN_SEPARATOR]) {
        Some(i) => (&text[..=i], &text[i + 1..]),
        None => ("", text),
    };
    let mut base = if dir_part.is_empty() {
        cwd.to_path_buf()
    } else {
        expand_tilde(dir_part)
    };
    if base.is_relative() {
        base = cwd.join(base);
    }
    let Ok(read) = std::fs::read_dir(&base) else {
        return (None, Vec::new());
    };
    let mut names: Vec<String> = read
        .filter_map(Result::ok)
        .filter(|e| e.path().is_dir())
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.starts_with(prefix) && (prefix.starts_with('.') || !n.starts_with('.')))
        .collect();
    names.sort();
    match names.as_slice() {
        [] => (None, Vec::new()),
        [only] => (Some(format!("{dir_part}{only}/")), vec![only.clone()]),
        many => {
            let mut common = many[0].clone();
            for n in &many[1..] {
                let len = common
                    .chars()
                    .zip(n.chars())
                    .take_while(|(a, b)| a == b)
                    .count();
                common = common.chars().take(len).collect();
            }
            let shown = many.iter().take(8).cloned().collect();
            (Some(format!("{dir_part}{common}")), shown)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn form(dir: &str) -> NewAgentForm {
        NewAgentForm::new(&AppConfig::default(), dir, Path::new("/"))
    }

    #[test]
    fn defaults_follow_the_config() {
        let f = form("/tmp");
        assert_eq!(f.provider, ProviderId::Claude);
        assert_eq!(f.model(), "sonnet");
        assert!(!f.yolo);
        assert_eq!(f.field, Field::Name);
        assert_eq!(f.dir.text(), "/tmp");
        assert!(!f.fields().contains(&Field::Effort));
    }

    #[test]
    fn provider_cycling_resets_the_model_and_shows_effort_for_codex() {
        let mut f = form("/tmp");
        f.field = Field::Provider;
        f.handle_key(&key(KeyCode::Right));
        assert_eq!(f.provider, ProviderId::Codex);
        assert_eq!(f.model(), "gpt-5.3-codex", "catalog default");
        assert!(f.fields().contains(&Field::Effort));
        f.field = Field::Effort;
        f.handle_key(&key(KeyCode::Right));
        f.handle_key(&key(KeyCode::Right));
        assert_eq!(f.effort(), Some("minimal"));
        f.field = Field::Provider;
        f.handle_key(&key(KeyCode::Right));
        assert_eq!(f.provider, ProviderId::Opencode);
        assert_eq!(f.effort(), None);
        assert_eq!(f.model(), "opencode/big-pickle");
    }

    #[test]
    fn model_cycles_and_accepts_free_text() {
        let mut f = form("/tmp");
        f.field = Field::Model;
        f.handle_key(&key(KeyCode::Right));
        assert_eq!(f.model(), "haiku");
        f.handle_key(&key(KeyCode::Char('x')));
        f.handle_key(&key(KeyCode::Char('y')));
        assert_eq!(f.model_idx, None);
        assert_eq!(f.model(), "xy");
        f.handle_key(&key(KeyCode::Left)); // cursor movement in free text
        assert_eq!(f.model(), "xy");
        f.handle_key(&KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL));
        assert_eq!(f.model(), "sonnet");
    }

    #[test]
    fn navigation_submit_and_cancel() {
        let mut f = form("/tmp");
        f.handle_key(&key(KeyCode::Tab));
        assert_eq!(f.field, Field::ProjectDir);
        f.handle_key(&key(KeyCode::Down));
        assert_eq!(f.field, Field::Provider);
        f.handle_key(&key(KeyCode::BackTab));
        assert_eq!(f.field, Field::ProjectDir);
        assert_eq!(f.handle_key(&key(KeyCode::Enter)), FormAction::Submit);
        assert_eq!(f.handle_key(&key(KeyCode::Esc)), FormAction::Cancel);
        f.field = Field::Prompt;
        f.handle_key(&key(KeyCode::Char('a')));
        f.handle_key(&KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT));
        f.handle_key(&key(KeyCode::Char('b')));
        assert_eq!(f.prompt.text(), "a\nb");
        f.field = Field::Yolo;
        f.handle_key(&key(KeyCode::Char(' ')));
        assert!(f.yolo);
    }

    #[test]
    fn builds_a_payload_and_validates_the_directory() {
        let dir = tempfile::tempdir().unwrap();
        let mut f = form(dir.path().to_str().unwrap());
        f.name.set_text("  api  ");
        f.prompt.set_text("do it\n");
        f.yolo = true;
        let p = f.build_payload().unwrap();
        assert_eq!(p.name, "api");
        assert_eq!(p.project_dir, dir.path().to_str().unwrap());
        assert_eq!(
            (p.provider, p.model.as_str()),
            (ProviderId::Claude, "sonnet")
        );
        assert_eq!(p.initial_prompt, "do it");
        assert!(p.yolo && p.reasoning_effort.is_none());

        f.dir.set_text("/definitely/not/here");
        assert!(f.build_payload().unwrap_err().contains("does not exist"));
        f.dir.set_text("  ");
        assert!(f.build_payload().unwrap_err().contains("required"));
    }

    #[test]
    fn relative_directories_resolve_against_the_cwd() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        let mut f = NewAgentForm::new(&AppConfig::default(), "sub", dir.path());
        assert!(f.build_payload().unwrap().project_dir.ends_with("sub"));
        f.dir.set_text("nope");
        assert!(f.build_payload().is_err());
    }

    #[test]
    fn path_completion() {
        let dir = tempfile::tempdir().unwrap();
        for d in ["alpha", "alps", "beta", ".hidden"] {
            std::fs::create_dir(dir.path().join(d)).unwrap();
        }
        std::fs::write(dir.path().join("alfile"), "").unwrap();
        let base = format!("{}/", dir.path().display());
        let (new, cands) = complete_path(&format!("{base}be"), Path::new("/"));
        assert_eq!(new, Some(format!("{base}beta/")));
        assert_eq!(cands, ["beta"]);
        let (new, cands) = complete_path(&format!("{base}al"), Path::new("/"));
        assert_eq!(new, Some(format!("{base}alp")), "common prefix only");
        assert_eq!(cands, ["alpha", "alps"]);
        let (_, cands) = complete_path(&base, Path::new("/"));
        assert_eq!(
            cands,
            ["alpha", "alps", "beta"],
            "hidden dirs and files skipped"
        );
        let (new, _) = complete_path(&format!("{base}.h"), Path::new("/"));
        assert_eq!(new, Some(format!("{base}.hidden/")));
        assert_eq!(
            complete_path("/nonexistent-dir-xyz/a", Path::new("/")),
            (None, vec![])
        );
    }

    #[test]
    fn tab_in_the_dir_field_completes_before_moving_on() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("proj")).unwrap();
        let mut f = form(&format!("{}/pr", dir.path().display()));
        f.field = Field::ProjectDir;
        f.handle_key(&key(KeyCode::Tab));
        assert_eq!(f.field, Field::ProjectDir);
        assert!(f.dir.text().ends_with("proj/"));
        f.handle_key(&key(KeyCode::Tab)); // nothing left to complete inside proj/
        assert_eq!(f.field, Field::Provider);
    }
}
