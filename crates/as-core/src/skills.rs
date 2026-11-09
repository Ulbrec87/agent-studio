//! Skills scan and toggle (port of Hydra's `SkillScanner.ts`).
//!
//! Claude: plugins listed in `<configDir>/plugins/installed_plugins.json` (enabled through
//! `enabledPlugins` of `<configDir>/settings.json`) plus user skills in `<configDir>/skills`.
//! Codex: `~/.agents/skills` (skill packs with a `skills/` dir or direct skills), `~/.codex/skills`
//! and the curated vendor import; a skill is disabled by renaming `SKILL.md` to `SKILL.md.disabled`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::types::ProviderId;

const ENABLED_FILE: &str = "SKILL.md";
const DISABLED_FILE: &str = "SKILL.md.disabled";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct SkillInfo {
    /// Directory name (Codex, user skills) — the toggle key of a Claude plugin is its `group`.
    pub id: String,
    pub name: String,
    pub description: String,
    pub provider: ProviderId,
    /// Plugin key (Claude), `user`, `agents`, `codex` or `curated`.
    pub group: String,
    pub enabled: bool,
    pub path: String,
    /// Claude profile the skill belongs to (empty for Codex).
    #[serde(default)]
    pub config_dir: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct SkillScan {
    pub claude: Vec<SkillInfo>,
    pub codex: Vec<SkillInfo>,
    pub opencode: Vec<SkillInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct SkillToggle {
    pub provider: ProviderId,
    pub enabled: bool,
    /// Claude: plugin key (`SkillInfo::group`) for plugin skills.
    #[serde(default)]
    pub group: String,
    #[serde(default)]
    pub config_dir: String,
    /// Codex: the skill file as listed by the scan.
    #[serde(default)]
    pub path: String,
}

#[derive(Debug, Clone)]
pub struct SkillScanOptions {
    pub claude_config_dirs: Vec<PathBuf>,
    pub home: PathBuf,
}

impl SkillScanOptions {
    /// Real locations: every `~/.claude*` profile (plus `$CLAUDE_CONFIG_DIR`) and the home dir.
    pub fn from_env() -> Self {
        let home = crate::paths::home_dir();
        let env = std::env::var("CLAUDE_CONFIG_DIR").ok();
        let dirs = crate::sessions::profiles::list_profiles(&home, env.as_deref())
            .into_iter()
            .map(|p| PathBuf::from(p.config_dir))
            .collect();
        Self {
            claude_config_dirs: dirs,
            home,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SkillError {
    #[error("skill not found")]
    NotFound,
    #[error("invalid skill file path")]
    InvalidPath,
    #[error("{0}")]
    Io(String),
    #[error("provider has no skills")]
    Unsupported,
}

/// `name:` and `description:` of the YAML front matter (no YAML parser: two single-line keys).
pub fn parse_frontmatter(content: &str) -> (String, String) {
    let content = content.trim_start_matches('\u{feff}').replace("\r\n", "\n");
    let Some(rest) = content.strip_prefix("---") else {
        return Default::default();
    };
    let Some(end) = rest.find("\n---") else {
        return Default::default();
    };
    let (mut name, mut desc) = (String::new(), String::new());
    for line in rest[..end].lines() {
        let unquote = |v: &str| v.trim().trim_matches(['"', '\'']).to_string();
        if let Some(v) = line.strip_prefix("name:") {
            name = unquote(v);
        } else if let Some(v) = line.strip_prefix("description:") {
            desc = unquote(v);
        }
    }
    (name, desc)
}

fn title_case(s: &str) -> String {
    s.replace(['-', '_'], " ")
        .split(' ')
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut c = w.chars();
            c.next()
                .map_or_else(String::new, |f| f.to_uppercase().chain(c).collect())
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn skill_file(dir: &Path) -> Option<(PathBuf, bool)> {
    let on = dir.join(ENABLED_FILE);
    if on.is_file() {
        return Some((on, true));
    }
    let off = dir.join(DISABLED_FILE);
    off.is_file().then_some((off, false))
}

fn read_skill(
    dir: &Path,
    provider: ProviderId,
    group: &str,
    config_dir: &str,
) -> Option<SkillInfo> {
    let (path, enabled) = skill_file(dir)?;
    let content = std::fs::read_to_string(&path).ok()?;
    let (name, description) = parse_frontmatter(&content);
    let id = dir.file_name()?.to_string_lossy().into_owned();
    Some(SkillInfo {
        name: if name.is_empty() {
            title_case(&id)
        } else {
            name
        },
        id,
        description,
        provider,
        group: group.to_string(),
        enabled,
        path: path.to_string_lossy().into_owned(),
        config_dir: config_dir.to_string(),
    })
}

fn sub_dirs(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    v.sort();
    v
}

fn scan_skill_dir(
    dir: &Path,
    provider: ProviderId,
    group: &str,
    config_dir: &str,
) -> Vec<SkillInfo> {
    sub_dirs(dir)
        .iter()
        .filter_map(|d| read_skill(d, provider, group, config_dir))
        .collect()
}

fn read_json_object(path: &Path) -> Map<String, Value> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| {
            if let Value::Object(m) = v {
                Some(m)
            } else {
                None
            }
        })
        .unwrap_or_default()
}

fn scan_claude_dir(config_dir: &Path) -> Vec<SkillInfo> {
    let cfg = config_dir.to_string_lossy().into_owned();
    let settings = read_json_object(&config_dir.join("settings.json"));
    let enabled_plugins = settings
        .get("enabledPlugins")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let installed = read_json_object(&config_dir.join("plugins").join("installed_plugins.json"));
    let mut out = Vec::new();
    if let Some(plugins) = installed.get("plugins").and_then(Value::as_object) {
        for (key, installs) in plugins {
            // Latest install; tolerate both an array of installs and a single object.
            let install_path = match installs {
                Value::Array(a) => a.last(),
                other => Some(other),
            }
            .and_then(|i| i.get("installPath"))
            .and_then(Value::as_str);
            let Some(install_path) = install_path else {
                continue;
            };
            let enabled = enabled_plugins.get(key).and_then(Value::as_bool) != Some(false);
            for mut s in scan_skill_dir(
                &Path::new(install_path).join("skills"),
                ProviderId::Claude,
                key,
                &cfg,
            ) {
                s.enabled = enabled;
                out.push(s);
            }
        }
    }
    out.extend(scan_skill_dir(
        &config_dir.join("skills"),
        ProviderId::Claude,
        "user",
        &cfg,
    ));
    out
}

fn codex_roots(home: &Path) -> Vec<(PathBuf, &'static str)> {
    vec![
        (home.join(".agents").join("skills"), "agents"),
        (home.join(".codex").join("skills"), "codex"),
        (
            home.join(".codex")
                .join("vendor_imports")
                .join("skills")
                .join("skills")
                .join(".curated"),
            "curated",
        ),
    ]
}

fn scan_codex(home: &Path) -> Vec<SkillInfo> {
    let mut out = Vec::new();
    for (root, group) in codex_roots(home) {
        if group == "curated" {
            out.extend(scan_skill_dir(&root, ProviderId::Codex, group, ""));
            continue;
        }
        for entry in sub_dirs(&root) {
            let pack = entry.join("skills");
            if pack.is_dir() {
                let name = entry
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                out.extend(scan_skill_dir(&pack, ProviderId::Codex, &name, ""));
            } else if let Some(s) = read_skill(&entry, ProviderId::Codex, group, "") {
                out.push(s);
            }
        }
    }
    out
}

pub fn scan(opts: &SkillScanOptions) -> SkillScan {
    SkillScan {
        claude: opts
            .claude_config_dirs
            .iter()
            .flat_map(|d| scan_claude_dir(d))
            .collect(),
        codex: scan_codex(&opts.home),
        opencode: Vec::new(),
    }
}

/// Write `value` to `path` atomically (temp file in the same dir + rename).
fn write_atomic(path: &Path, text: &str) -> std::io::Result<()> {
    let tmp = path.with_extension(format!("json.tmp{}", std::process::id()));
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

pub fn toggle(opts: &SkillScanOptions, req: &SkillToggle) -> Result<(), SkillError> {
    match req.provider {
        ProviderId::Claude => toggle_claude(opts, req),
        ProviderId::Codex => toggle_codex(opts, req),
        ProviderId::Opencode => Err(SkillError::Unsupported),
    }
}

fn toggle_claude(opts: &SkillScanOptions, req: &SkillToggle) -> Result<(), SkillError> {
    if req.group.is_empty() || req.group == "user" {
        return Err(SkillError::NotFound);
    }
    let dir = PathBuf::from(&req.config_dir);
    // Only profiles the scan knows about may be edited.
    if !opts.claude_config_dirs.iter().any(|d| d == &dir) {
        return Err(SkillError::InvalidPath);
    }
    let path = dir.join("settings.json");
    let mut settings = read_json_object(&path);
    let mut plugins = settings
        .get("enabledPlugins")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    plugins.insert(req.group.clone(), Value::Bool(req.enabled));
    settings.insert("enabledPlugins".into(), Value::Object(plugins));
    let text = serde_json::to_string_pretty(&Value::Object(settings))
        .map_err(|e| SkillError::Io(e.to_string()))?;
    write_atomic(&path, &text).map_err(|e| SkillError::Io(e.to_string()))
}

fn toggle_codex(opts: &SkillScanOptions, req: &SkillToggle) -> Result<(), SkillError> {
    let file = PathBuf::from(&req.path);
    let name = file
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    if name != ENABLED_FILE && name != DISABLED_FILE {
        return Err(SkillError::InvalidPath);
    }
    // The file must be one the scan lists (no renaming of arbitrary paths).
    let known = scan_codex(&opts.home)
        .iter()
        .any(|s| Path::new(&s.path) == file);
    if !known {
        return Err(SkillError::NotFound);
    }
    let dir = file.parent().ok_or(SkillError::InvalidPath)?;
    let (on, off) = (dir.join(ENABLED_FILE), dir.join(DISABLED_FILE));
    let io = |e: std::io::Error| SkillError::Io(e.to_string());
    if req.enabled && off.exists() {
        std::fs::rename(&off, &on).map_err(io)?;
    } else if !req.enabled && on.exists() {
        std::fs::rename(&on, &off).map_err(io)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(p: &Path, s: &str) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, s).unwrap();
    }

    fn fixture() -> (tempfile::TempDir, SkillScanOptions) {
        let d = tempfile::tempdir().unwrap();
        let home = d.path().to_path_buf();
        let claude = home.join(".claude");
        let plugin = home.join("plug-install");
        write(
            &plugin.join("skills").join("brainstorm").join("SKILL.md"),
            "---\nname: Brainstorm\ndescription: \"Think first\"\n---\nbody",
        );
        write(
            &plugin
                .join("skills")
                .join("no-skill-here")
                .join("README.md"),
            "x",
        );
        write(
            &claude.join("plugins").join("installed_plugins.json"),
            &format!(
                "{{\"plugins\":{{\"sp@market\":[{{\"installPath\":{}}}]}}}}",
                serde_json::to_string(&plugin).unwrap()
            ),
        );
        write(
            &claude.join("settings.json"),
            "{\"model\":\"opus\",\"enabledPlugins\":{\"sp@market\":false,\"other\":true}}",
        );
        write(
            &claude.join("skills").join("my-skill").join("SKILL.md"),
            "no front matter",
        );
        write(
            &home
                .join(".agents")
                .join("skills")
                .join("pack")
                .join("skills")
                .join("tdd")
                .join("SKILL.md"),
            "---\nname: tdd\n---\n",
        );
        write(
            &home
                .join(".agents")
                .join("skills")
                .join("direct")
                .join("SKILL.md.disabled"),
            "---\ndescription: d\n---\n",
        );
        write(
            &home
                .join(".codex")
                .join("skills")
                .join("cx")
                .join("SKILL.md"),
            "---\nname: cx\n---\n",
        );
        (
            d,
            SkillScanOptions {
                claude_config_dirs: vec![claude],
                home,
            },
        )
    }

    #[test]
    fn frontmatter_is_tolerant() {
        assert_eq!(
            parse_frontmatter("---\nname: a b\ndescription: 'x'\n---\n"),
            ("a b".into(), "x".into())
        );
        assert_eq!(parse_frontmatter("---\r\nname: win\r\n---\r\n").0, "win");
        assert_eq!(parse_frontmatter("nothing"), Default::default());
        assert_eq!(
            parse_frontmatter("---\nname: unterminated"),
            Default::default()
        );
    }

    #[test]
    fn scans_claude_and_codex() {
        let (_d, opts) = fixture();
        let scan = scan(&opts);
        let ids: Vec<_> = scan
            .claude
            .iter()
            .map(|s| (s.id.as_str(), s.group.as_str(), s.enabled))
            .collect();
        assert_eq!(
            ids,
            [
                ("brainstorm", "sp@market", false),
                ("my-skill", "user", true)
            ]
        );
        assert_eq!(scan.claude[0].name, "Brainstorm");
        assert_eq!(scan.claude[0].description, "Think first");
        assert_eq!(scan.claude[1].name, "My Skill");
        let codex: Vec<_> = scan
            .codex
            .iter()
            .map(|s| (s.id.as_str(), s.group.as_str(), s.enabled))
            .collect();
        assert_eq!(
            codex,
            [
                ("direct", "agents", false),
                ("tdd", "pack", true),
                ("cx", "codex", true)
            ]
        );
    }

    #[test]
    fn toggles_claude_preserving_other_keys() {
        let (_d, opts) = fixture();
        let cfg = opts.claude_config_dirs[0].clone();
        let req = SkillToggle {
            provider: ProviderId::Claude,
            enabled: true,
            group: "sp@market".into(),
            config_dir: cfg.to_string_lossy().into_owned(),
            path: String::new(),
        };
        toggle(&opts, &req).unwrap();
        let v: Value =
            serde_json::from_str(&std::fs::read_to_string(cfg.join("settings.json")).unwrap())
                .unwrap();
        assert_eq!(v["model"], "opus");
        assert_eq!(v["enabledPlugins"]["sp@market"], true);
        assert_eq!(v["enabledPlugins"]["other"], true);
        assert!(scan(&opts).claude[0].enabled);
        let bad = SkillToggle {
            config_dir: "/elsewhere".into(),
            ..req
        };
        assert!(matches!(toggle(&opts, &bad), Err(SkillError::InvalidPath)));
    }

    #[test]
    fn toggles_codex_by_renaming() {
        let (_d, opts) = fixture();
        let tdd = scan(&opts)
            .codex
            .into_iter()
            .find(|s| s.id == "tdd")
            .unwrap();
        let req = SkillToggle {
            provider: ProviderId::Codex,
            enabled: false,
            group: String::new(),
            config_dir: String::new(),
            path: tdd.path.clone(),
        };
        toggle(&opts, &req).unwrap();
        assert!(!Path::new(&tdd.path).exists());
        assert!(Path::new(&tdd.path).with_file_name(DISABLED_FILE).exists());
        let disabled = scan(&opts)
            .codex
            .into_iter()
            .find(|s| s.id == "tdd")
            .unwrap();
        assert!(!disabled.enabled);
        toggle(
            &opts,
            &SkillToggle {
                enabled: true,
                path: disabled.path,
                ..req.clone()
            },
        )
        .unwrap();
        assert!(Path::new(&tdd.path).exists());
        // Paths the scan does not list, or that are not skill files, are refused.
        assert!(matches!(
            toggle(
                &opts,
                &SkillToggle {
                    path: opts.home.join("x").to_string_lossy().into_owned(),
                    ..req.clone()
                }
            ),
            Err(SkillError::InvalidPath)
        ));
        let other = opts.home.join("x").join("SKILL.md");
        assert!(matches!(
            toggle(
                &opts,
                &SkillToggle {
                    path: other.to_string_lossy().into_owned(),
                    ..req
                }
            ),
            Err(SkillError::NotFound)
        ));
    }
}
