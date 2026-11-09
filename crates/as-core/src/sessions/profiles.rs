//! Claude Code profiles: several `CLAUDE_CONFIG_DIR`s side by side (Hydra
//! `electron/sessions/claudeProfiles.ts` + `shared/claudeProfiles.ts`).
//!
//! A profile is a Claude config dir: the default `~/.claude`, `$CLAUDE_CONFIG_DIR` when set, and
//! alternates kept as `~/.claude-<name>`. A project can be mapped to a profile
//! (`claudeProfileByProject`; `""` = the default profile); subfolders inherit and the longest
//! mapped path wins.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::transcript::find_claude_transcript_file;
use crate::paths::home_dir;

/// A Claude config dir the user can pick.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase", default)]
pub struct ClaudeProfile {
    pub config_dir: String,
    /// `default`, or the `~/.claude-<name>` suffix.
    pub label: String,
    pub is_default: bool,
}

fn absolute(path: &str) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| PathBuf::from(path))
}

/// Every Claude config dir on this machine, default first; missing dirs are skipped.
pub fn discover_config_dirs(home: &Path, env_config_dir: Option<&str>) -> Vec<PathBuf> {
    let mut dirs = vec![home.join(".claude")];
    if let Some(custom) = env_config_dir.filter(|d| !d.is_empty()) {
        dirs.push(absolute(custom));
    }
    if let Ok(read) = std::fs::read_dir(home) {
        let mut extra: Vec<PathBuf> = read
            .filter_map(Result::ok)
            .filter(|e| {
                e.file_type().is_ok_and(|t| t.is_dir())
                    && e.file_name()
                        .to_string_lossy()
                        .strip_prefix(".claude-")
                        .is_some_and(|rest| !rest.is_empty())
            })
            .map(|e| e.path())
            .collect();
        extra.sort();
        dirs.extend(extra);
    }
    let mut seen = Vec::new();
    dirs.into_iter()
        .filter(|d| {
            if seen.contains(d) {
                return false;
            }
            seen.push(d.clone());
            d.exists()
        })
        .collect()
}

/// The `projects` dir of every profile, where transcripts live.
pub fn discover_projects_dirs(home: &Path, env_config_dir: Option<&str>) -> Vec<PathBuf> {
    discover_config_dirs(home, env_config_dir)
        .into_iter()
        .map(|d| d.join("projects"))
        .filter(|d| d.exists())
        .collect()
}

/// [`discover_projects_dirs`] for the real home dir and `$CLAUDE_CONFIG_DIR`.
pub fn discover_projects_dirs_from_env() -> Vec<PathBuf> {
    let env = std::env::var("CLAUDE_CONFIG_DIR").ok();
    discover_projects_dirs(&home_dir(), env.as_deref())
}

/// Profiles the user can pick from, labelled by their `~/.claude-<name>` suffix.
pub fn list_profiles(home: &Path, env_config_dir: Option<&str>) -> Vec<ClaudeProfile> {
    let default_dir = home.join(".claude");
    discover_config_dirs(home, env_config_dir)
        .into_iter()
        .map(|dir| {
            let name = dir
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let is_default = dir == default_dir;
            let label = if is_default {
                "default".to_string()
            } else {
                name.strip_prefix(".claude-").unwrap_or(&name).to_string()
            };
            ClaudeProfile {
                config_dir: dir.to_string_lossy().into_owned(),
                label,
                is_default,
            }
        })
        .collect()
}

/// [`list_profiles`] for the real home dir and `$CLAUDE_CONFIG_DIR`.
pub fn list_profiles_from_env() -> Vec<ClaudeProfile> {
    let env = std::env::var("CLAUDE_CONFIG_DIR").ok();
    list_profiles(&home_dir(), env.as_deref())
}

/// The profile a transcript belongs to: `<configDir>/projects/<project>/<id>.jsonl`.
/// `""` for the default `~/.claude` profile, matching the project mapping.
pub fn config_dir_for_transcript(transcript: &Path, home: &Path) -> String {
    let config_dir = transcript
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .unwrap_or(Path::new(""));
    if config_dir == home.join(".claude") {
        String::new()
    } else {
        config_dir.to_string_lossy().into_owned()
    }
}

fn normalize_dir(dir: &str) -> String {
    dir.replace('\\', "/").trim_end_matches('/').to_string()
}

/// The config dir mapped to `project_dir`, matching the project itself or any parent; the longest
/// mapped path wins. `Some("")` means the default profile, `None` no mapping (the agent keeps the
/// daemon's environment).
pub fn resolve_profile(
    project_dir: &str,
    profile_by_project: &BTreeMap<String, String>,
) -> Option<String> {
    let target = normalize_dir(project_dir);
    let mut best: Option<(usize, &String)> = None;
    for (mapped, config_dir) in profile_by_project {
        let prefix = normalize_dir(mapped);
        if prefix.is_empty() {
            continue;
        }
        let matches = target == prefix || target.starts_with(&format!("{prefix}/"));
        if matches && best.is_none_or(|(len, _)| prefix.len() > len) {
            best = Some((prefix.len(), config_dir));
        }
    }
    best.map(|(_, c)| c.clone())
}

/// A copy of the mapping with `project_dir` set to `config_dir`, or removed when `None`.
pub fn with_profile(
    profile_by_project: &BTreeMap<String, String>,
    project_dir: &str,
    config_dir: Option<&str>,
) -> BTreeMap<String, String> {
    let mut next = profile_by_project.clone();
    match config_dir {
        None => next.remove(project_dir),
        Some(dir) => next.insert(project_dir.to_string(), dir.to_string()),
    };
    next
}

/// Display name of a mapped config dir: the profile label, or the raw path when it no longer exists.
pub fn profile_label(config_dir: &str, profiles: &[ClaudeProfile]) -> String {
    if config_dir.is_empty() {
        return profiles
            .iter()
            .find(|p| p.is_default)
            .map_or("default", |p| p.label.as_str())
            .to_string();
    }
    profiles
        .iter()
        .find(|p| p.config_dir == config_dir)
        .map_or(config_dir, |p| p.label.as_str())
        .to_string()
}

/// The profile a resumed session lives in: the one that owns its transcript (`None` when the
/// transcript is not found).
pub fn profile_of_session(
    session_id: &str,
    projects_dirs: &[PathBuf],
    home: &Path,
) -> Option<String> {
    projects_dirs
        .iter()
        .find_map(|dir| find_claude_transcript_file(session_id, dir))
        .map(|path| config_dir_for_transcript(&path, home))
}

/// Resolver for the manager (`(project_dir, session_id) -> CLAUDE_CONFIG_DIR`): a resumed session
/// only exists in the profile that saved it, so that wins over the project mapping; new sessions
/// follow the project (or nearest parent) mapping. Profiles are rediscovered on every call so new
/// ones show up without a restart.
pub fn make_resolver(
    get_map: impl Fn() -> BTreeMap<String, String> + Send + Sync + 'static,
) -> impl Fn(&str, Option<&str>) -> Option<String> + Send + Sync + 'static {
    move |project_dir, session_id| {
        if let Some(sid) = session_id {
            let home = home_dir();
            let dirs = discover_projects_dirs_from_env();
            if let Some(found) = profile_of_session(sid, &dirs, &home) {
                return Some(found);
            }
        }
        resolve_profile(project_dir, &get_map())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sessions::testutil::write;
    use pretty_assertions::assert_eq;

    /// Compare by components, not by text: `h.join(".claude/projects")` keeps a `/` on Windows.
    fn strs(paths: &[PathBuf]) -> Vec<Vec<String>> {
        paths
            .iter()
            .map(|p| {
                p.components()
                    .map(|c| c.as_os_str().to_string_lossy().into_owned())
                    .collect()
            })
            .collect()
    }

    #[test]
    fn finds_projects_dirs_of_every_profile() {
        let home = tempfile::tempdir().unwrap();
        let custom = tempfile::tempdir().unwrap();
        let h = home.path();
        std::fs::create_dir_all(h.join(".claude/projects")).unwrap();
        std::fs::create_dir_all(h.join(".claude-work/projects")).unwrap();
        std::fs::create_dir_all(h.join(".claude-alternative/projects")).unwrap();
        std::fs::create_dir_all(h.join(".claude-empty")).unwrap();
        std::fs::create_dir_all(custom.path().join("projects")).unwrap();
        write(&h.join(".claude-notes"), "");

        let dirs = discover_projects_dirs(h, custom.path().to_str());
        assert_eq!(dirs[0], h.join(".claude/projects"), "default first");
        let mut got = strs(&dirs);
        got.sort();
        let mut want = strs(&[
            h.join(".claude/projects"),
            h.join(".claude-alternative/projects"),
            h.join(".claude-work/projects"),
            custom.path().join("projects"),
        ]);
        want.sort();
        assert_eq!(got, want);
    }

    #[test]
    fn does_not_list_a_profile_twice_when_the_env_dir_points_at_one() {
        let home = tempfile::tempdir().unwrap();
        let h = home.path();
        std::fs::create_dir_all(h.join(".claude-work/projects")).unwrap();
        let dirs = discover_projects_dirs(h, h.join(".claude-work").to_str());
        assert_eq!(dirs, vec![h.join(".claude-work/projects")]);
    }

    #[test]
    fn labels_profiles_by_their_suffix_including_empty_ones() {
        let home = tempfile::tempdir().unwrap();
        let h = home.path();
        std::fs::create_dir_all(h.join(".claude")).unwrap();
        std::fs::create_dir_all(h.join(".claude-work")).unwrap();
        assert_eq!(
            list_profiles(h, None),
            vec![
                ClaudeProfile {
                    config_dir: h.join(".claude").to_string_lossy().into_owned(),
                    label: "default".into(),
                    is_default: true
                },
                ClaudeProfile {
                    config_dir: h.join(".claude-work").to_string_lossy().into_owned(),
                    label: "work".into(),
                    is_default: false
                },
            ]
        );
    }

    #[test]
    fn maps_a_transcript_path_back_to_its_profile() {
        let home = Path::new("/home/me");
        assert_eq!(
            config_dir_for_transcript(
                Path::new("/home/me/.claude-work/projects/-var-www-test/abc.jsonl"),
                home
            ),
            "/home/me/.claude-work"
        );
        assert_eq!(
            config_dir_for_transcript(Path::new("/home/me/.claude/projects/-tmp/abc.jsonl"), home),
            ""
        );
    }

    #[test]
    fn a_resumed_session_uses_the_profile_that_saved_it() {
        let home = tempfile::tempdir().unwrap();
        let h = home.path();
        write(
            &h.join(".claude-work/projects/-var-www-test/session-work.jsonl"),
            "{}\n",
        );
        write(&h.join(".claude/projects/-tmp/session-def.jsonl"), "{}\n");
        let dirs = vec![h.join(".claude/projects"), h.join(".claude-work/projects")];
        assert_eq!(
            profile_of_session("session-work", &dirs, h),
            Some(h.join(".claude-work").to_string_lossy().into_owned())
        );
        assert_eq!(
            profile_of_session("session-def", &dirs, h),
            Some(String::new())
        );
        assert_eq!(profile_of_session("nope", &dirs, h), None);
    }

    fn map() -> BTreeMap<String, String> {
        [
            ("/var/www/test", "/home/me/.claude-work"),
            ("/home/me/projects/", "/home/me/.claude-alternative"),
            ("/home/me/projects/personal", ""),
        ]
        .into_iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect()
    }

    #[test]
    fn resolve_matches_the_project_and_its_subfolders() {
        let m = map();
        assert_eq!(
            resolve_profile("/var/www/test", &m).as_deref(),
            Some("/home/me/.claude-work")
        );
        assert_eq!(
            resolve_profile("/var/www/test/app/", &m).as_deref(),
            Some("/home/me/.claude-work")
        );
    }

    #[test]
    fn resolve_ignores_siblings_sharing_a_name_prefix() {
        assert_eq!(resolve_profile("/var/www/test-2", &map()), None);
    }

    #[test]
    fn resolve_prefers_the_longest_path_including_an_explicit_default() {
        let m = map();
        assert_eq!(
            resolve_profile("/home/me/projects/client", &m).as_deref(),
            Some("/home/me/.claude-alternative")
        );
        assert_eq!(
            resolve_profile("/home/me/projects/personal/blog", &m).as_deref(),
            Some("")
        );
    }

    #[test]
    fn resolve_without_a_mapping() {
        assert_eq!(resolve_profile("/tmp", &map()), None);
        assert_eq!(resolve_profile("/tmp", &BTreeMap::new()), None);
    }

    #[test]
    fn with_profile_sets_and_removes() {
        let m = with_profile(&BTreeMap::new(), "/a", Some("/x"));
        assert_eq!(m.get("/a").map(String::as_str), Some("/x"));
        assert!(with_profile(&m, "/a", None).is_empty());
    }

    #[test]
    fn labels_for_mapped_dirs() {
        let profiles = vec![
            ClaudeProfile {
                config_dir: "/h/.claude".into(),
                label: "default".into(),
                is_default: true,
            },
            ClaudeProfile {
                config_dir: "/h/.claude-work".into(),
                label: "work".into(),
                is_default: false,
            },
        ];
        assert_eq!(profile_label("", &profiles), "default");
        assert_eq!(profile_label("/h/.claude-work", &profiles), "work");
        assert_eq!(profile_label("/gone", &profiles), "/gone");
    }
}
