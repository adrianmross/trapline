//! nudge: rules in TOML, evaluated against a trapline [`Event`].
//!
//! Conditions are a fixed set of named checks (no scripting), so a rules file
//! is safe to read. Actions escalate from `hint`/`warn` to `block`/`wrap`.

use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use trapline_core::{Decision, Event, Handler, Level, Message, Phase};

const DEVENV_PACK: &str = include_str!("../packs/devenv.toml");

/// Built-in packs by name.
pub fn builtin_pack(name: &str) -> Option<&'static str> {
    match name {
        "devenv" => Some(DEVENV_PACK),
        _ => None,
    }
}

pub const BUILTIN_PACKS: &[&str] = &["devenv"];

#[derive(Debug, Default, Deserialize)]
pub struct Config {
    /// Built-in packs to load, by name.
    #[serde(default, rename = "use")]
    pub packs: Vec<String>,
    #[serde(default)]
    pub rule: Vec<Rule>,
}

#[derive(Debug, Deserialize)]
pub struct Rule {
    pub id: String,
    #[serde(default)]
    pub tool: Vec<String>,
    pub phase: Phase,
    #[serde(default)]
    pub when: When,
    #[serde(rename = "do")]
    pub action: Action,
}

#[derive(Debug, Default, Deserialize)]
pub struct When {
    /// A file exists at or above the cwd.
    pub root_has: Option<String>,
    pub env_set: Option<String>,
    pub env_unset: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct Action {
    pub hint: Option<String>,
    pub warn: Option<String>,
    #[serde(default)]
    pub block: bool,
    #[serde(default)]
    pub wrap: Vec<String>,
    #[serde(default)]
    pub set_env: BTreeMap<String, String>,
}

pub struct Engine {
    rules: Vec<Rule>,
}

impl Engine {
    /// Parse a config and expand its `use = [...]` packs. Unknown packs error.
    pub fn from_toml(src: &str) -> Result<Engine, String> {
        let cfg: Config = toml::from_str(src).map_err(|e| e.to_string())?;
        let mut rules = vec![];
        for name in &cfg.packs {
            let pack = builtin_pack(name).ok_or_else(|| format!("unknown pack: {name}"))?;
            let p: Config = toml::from_str(pack).map_err(|e| format!("pack {name}: {e}"))?;
            rules.extend(p.rule);
        }
        rules.extend(cfg.rule);
        Ok(Engine { rules })
    }

    /// Tools with `pre`/`post` rules: the shell glue defines wrappers for these.
    pub fn wrapped_tools(&self) -> BTreeSet<&str> {
        self.rules
            .iter()
            .filter(|r| matches!(r.phase, Phase::Pre | Phase::Post))
            .flat_map(|r| r.tool.iter().map(String::as_str))
            .collect()
    }

    fn applies(rule: &Rule, ev: &Event) -> bool {
        if rule.phase != ev.phase {
            return false;
        }
        if !rule.tool.is_empty() && !ev.tool.as_ref().is_some_and(|t| rule.tool.contains(t)) {
            return false;
        }
        let w = &rule.when;
        w.root_has.as_deref().is_none_or(|f| root_has(&ev.cwd, f))
            && w.env_set.as_deref().is_none_or(|k| ev.env.contains_key(k))
            && w.env_unset
                .as_deref()
                .is_none_or(|k| !ev.env.contains_key(k))
    }
}

fn root_has(cwd: &Path, file: &str) -> bool {
    cwd.ancestors().any(|d| d.join(file).is_file())
}

impl Handler for Engine {
    fn handle(&self, ev: &Event) -> Decision {
        let mut d = Decision::default();
        // Every key a prompt rule can set starts unset, so stale markers clear.
        if ev.phase == Phase::Prompt {
            for r in self.rules.iter().filter(|r| r.phase == Phase::Prompt) {
                for k in r.action.set_env.keys() {
                    d.set_env.insert(k.clone(), None);
                }
            }
        }
        for r in self.rules.iter().filter(|r| Self::applies(r, ev)) {
            d.fired.push(r.id.clone());
            let a = &r.action;
            let subject = ev
                .tool
                .as_deref()
                .map(|t| format!("{t} "))
                .unwrap_or_default();
            if let Some(t) = &a.hint {
                d.messages.push(Message {
                    level: Level::Hint,
                    text: format!("{subject}{t}"),
                });
            }
            if let Some(t) = &a.warn {
                d.messages.push(Message {
                    level: Level::Warn,
                    text: format!("{subject}{t}"),
                });
            }
            if a.block {
                d.proceed = false;
            }
            if d.prefix.is_empty() && !a.wrap.is_empty() {
                d.prefix = a.wrap.clone();
            }
            for (k, v) in &a.set_env {
                d.set_env.insert(k.clone(), Some(v.clone()));
            }
        }
        d
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn ev(phase: Phase, tool: Option<&str>, cwd: &Path, env: &[(&str, &str)]) -> Event {
        Event {
            phase,
            tool: tool.map(String::from),
            args: vec![],
            cwd: cwd.to_path_buf(),
            env: env
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            rc: None,
        }
    }

    fn repo(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("nudge-{name}-{}", std::process::id()));
        fs::create_dir_all(d.join("sub")).unwrap();
        fs::write(d.join("devenv.nix"), "").unwrap();
        d
    }

    #[test]
    fn devenv_pack_wraps_nvim_until_entered() {
        let e = Engine::from_toml(r#"use = ["devenv"]"#).unwrap();
        let cwd = repo("wrap").join("sub");
        let out = e.handle(&ev(Phase::Pre, Some("nvim"), &cwd, &[]));
        assert_eq!(out.prefix[..2], ["devenv", "shell"]);
        let inside = e.handle(&ev(
            Phase::Pre,
            Some("nvim"),
            &cwd,
            &[("DEVENV_ROOT", "/x")],
        ));
        assert!(inside.prefix.is_empty());
        let other = e.handle(&ev(Phase::Pre, Some("git"), &cwd, &[]));
        assert!(other.prefix.is_empty());
    }

    #[test]
    fn prompt_sets_one_marker_and_clears_the_other() {
        let e = Engine::from_toml(r#"use = ["devenv"]"#).unwrap();
        let cwd = repo("prompt");
        let off = e.handle(&ev(Phase::Prompt, None, &cwd, &[]));
        assert_eq!(off.set_env["DEVENV_OFF"].as_deref(), Some("⬡ off"));
        assert_eq!(off.set_env["DEVENV_ON"], None);
        let on = e.handle(&ev(Phase::Prompt, None, &cwd, &[("DEVENV_ROOT", "/x")]));
        assert_eq!(on.set_env["DEVENV_ON"].as_deref(), Some("⬡ devenv"));
        assert_eq!(on.set_env["DEVENV_OFF"], None);
    }

    #[test]
    fn block_and_unknown_pack() {
        let e = Engine::from_toml(
            r#"[[rule]]
id = "no-rm"
tool = ["rm"]
phase = "pre"
do = { warn = "careful", block = true }"#,
        )
        .unwrap();
        let d = e.handle(&ev(Phase::Pre, Some("rm"), Path::new("/"), &[]));
        assert!(!d.proceed);
        assert_eq!(d.messages[0].text, "rm careful");
        assert!(Engine::from_toml(r#"use = ["nope"]"#).is_err());
    }
}
