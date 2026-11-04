//! Environment for spawned agent CLIs (Hydra `electron/agents/agentEnv.ts`).

use std::collections::{BTreeMap, HashMap};

/// Variables a running Claude Code session sets for its own children. When the
/// daemon is launched from inside a Claude Code session they leak into every
/// agent, and `CLAUDE_CODE_CHILD_SESSION` in particular makes Claude treat the
/// agent as a sub-session and stop saving its transcript. User-level settings
/// such as `CLAUDE_CONFIG_DIR` or `CLAUDE_CODE_USE_BEDROCK` are kept.
pub const PARENT_SESSION_ENV_VARS: [&str; 11] = [
    "CLAUDECODE",
    "CLAUDE_PID",
    "CLAUDE_EFFORT",
    "CLAUDE_JOB_DIR",
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_CODE_SESSION_ATTENDED",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_EXECPATH",
    "CLAUDE_CODE_MESSAGING_SOCKET",
    "CLAUDE_CODE_MESSAGING_TOKEN",
];

/// Env overrides: `Some(value)` sets, `None` removes the variable.
pub type EnvOverrides = BTreeMap<String, Option<String>>;

/// Environment for an agent CLI: `base` minus parent-session markers, plus `overrides`.
pub fn build_agent_env(
    overrides: &EnvOverrides,
    base: &HashMap<String, String>,
) -> HashMap<String, String> {
    let mut env = base.clone();
    for key in PARENT_SESSION_ENV_VARS {
        env.remove(key);
    }
    for (key, value) in overrides {
        match value {
            Some(v) => {
                env.insert(key.clone(), v.clone());
            }
            None => {
                env.remove(key);
            }
        }
    }
    env
}

/// The current process environment (non-Unicode entries are skipped).
pub fn process_env() -> HashMap<String, String> {
    std::env::vars_os()
        .filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?)))
        .collect()
}

/// [`build_agent_env`] on the current process environment.
pub fn build_agent_env_from_process(overrides: &EnvOverrides) -> HashMap<String, String> {
    build_agent_env(overrides, &process_env())
}

/// Overrides that select a Claude profile: a config dir sets `CLAUDE_CONFIG_DIR`,
/// `Some("")` clears it so the default `~/.claude` is used, and `None` leaves the
/// daemon's env as is.
pub fn claude_profile_env(config_dir: Option<&str>) -> EnvOverrides {
    let mut out = EnvOverrides::new();
    if let Some(dir) = config_dir {
        out.insert(
            "CLAUDE_CONFIG_DIR".into(),
            (!dir.is_empty()).then(|| dir.to_string()),
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn map(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }
    fn over(pairs: &[(&str, Option<&str>)]) -> EnvOverrides {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), v.map(str::to_string)))
            .collect()
    }

    #[test]
    fn drops_markers_leaked_from_a_parent_claude_code_session() {
        let env = build_agent_env(
            &over(&[("FORCE_COLOR", Some("1"))]),
            &map(&[
                ("PATH", "/usr/bin"),
                ("CLAUDECODE", "1"),
                ("CLAUDE_CODE_CHILD_SESSION", "1"),
                ("CLAUDE_CODE_SESSION_ID", "abc"),
                ("CLAUDE_CODE_ENTRYPOINT", "cli"),
                ("CLAUDE_JOB_DIR", "/home/me/.claude/jobs/abc"),
            ]),
        );
        assert_eq!(env, map(&[("PATH", "/usr/bin"), ("FORCE_COLOR", "1")]));
    }

    #[test]
    fn keeps_user_level_claude_settings() {
        let base = map(&[
            ("CLAUDE_CONFIG_DIR", "/home/me/.claude-work"),
            ("CLAUDE_CODE_USE_BEDROCK", "1"),
            ("ANTHROPIC_MODEL", "opus"),
        ]);
        assert_eq!(build_agent_env(&EnvOverrides::new(), &base), base);
    }

    #[test]
    fn lets_overrides_win() {
        assert_eq!(
            build_agent_env(
                &over(&[("TERM", Some("xterm-256color"))]),
                &map(&[("TERM", "dumb")])
            ),
            map(&[("TERM", "xterm-256color")])
        );
    }

    #[test]
    fn removes_variables_overridden_with_none() {
        assert_eq!(
            build_agent_env(
                &over(&[("CLAUDE_CONFIG_DIR", None)]),
                &map(&[("CLAUDE_CONFIG_DIR", "/x"), ("PATH", "/bin")])
            ),
            map(&[("PATH", "/bin")])
        );
    }

    #[test]
    fn claude_profile_env_sets_clears_or_leaves_config_dir_alone() {
        let base = map(&[("CLAUDE_CONFIG_DIR", "/home/me/.claude-alternative")]);
        assert_eq!(
            build_agent_env(&claude_profile_env(Some("/home/me/.claude-work")), &base),
            map(&[("CLAUDE_CONFIG_DIR", "/home/me/.claude-work")])
        );
        assert_eq!(
            build_agent_env(&claude_profile_env(Some("")), &base),
            map(&[])
        );
        assert_eq!(build_agent_env(&claude_profile_env(None), &base), base);
    }
}
