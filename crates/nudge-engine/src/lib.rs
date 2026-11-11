//! nudge: rules in TOML, evaluated against a trapline [`Event`].
//!
//! Conditions are a fixed set of named checks (no scripting), so a rules file
//! is safe to read. Actions escalate from `hint`/`warn` to `block`/`wrap`.

use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use trapline_core::{Decision, Event, Handler, Level, Message, Phase};

const DEVENV_PACK: &str = include_str!("../packs/devenv.toml");
const GIT_PACK: &str = include_str!("../packs/git.toml");
const WT_PACK: &str = include_str!("../packs/wt.toml");
const DEVENV_AGENT_PACK: &str = include_str!("../packs/devenv-agent.toml");

/// Env vars that mark the caller as an agent. `AGENT=1` is the generic switch.
const AGENT_VARS: &[&str] = &["AGENT", "CLAUDECODE"];

/// Built-in packs by name.
pub fn builtin_pack(name: &str) -> Option<&'static str> {
    match name {
        "devenv" => Some(DEVENV_PACK),
        "git" => Some(GIT_PACK),
        "wt" => Some(WT_PACK),
        "devenv-agent" => Some(DEVENV_AGENT_PACK),
        _ => None,
    }
}

pub const BUILTIN_PACKS: &[&str] = &["devenv", "devenv-agent", "git", "wt"];

#[derive(Debug, Default, Deserialize)]
pub struct Config {
    /// Built-in packs to load, by name.
    #[serde(default, rename = "use")]
    pub packs: Vec<String>,
    #[serde(default)]
    pub rule: Vec<Rule>,
    /// External handlers: any program that reads an Event as JSON on stdin and
    /// prints a Decision as JSON on stdout.
    #[serde(default)]
    pub handler: Vec<HandlerCfg>,
}

#[derive(Debug, Deserialize)]
pub struct HandlerCfg {
    pub id: String,
    /// argv; the first element is the program.
    pub command: Vec<String>,
    /// Only called for these phases / tools (empty = all).
    #[serde(default)]
    pub phase: Vec<Phase>,
    #[serde(default)]
    pub tool: Vec<String>,
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
}

/// Generous: macOS scans a never-run executable on its first exec (~1s).
fn default_timeout_ms() -> u64 {
    2000
}

#[derive(Debug, Deserialize)]
pub struct Rule {
    pub id: String,
    #[serde(default)]
    pub tool: Vec<String>,
    /// The first positional argument is one of these (after global flags).
    #[serde(default)]
    pub sub: Vec<String>,
    /// The positional arguments start with these (after global flags).
    #[serde(default)]
    pub args_start: Vec<String>,
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
    /// Git state read from `.git`, no process spawned.
    pub git: Option<GitCond>,
    /// The caller is (true) or is not (false) an agent.
    pub agent: Option<bool>,
    /// This command is not found on `$PATH`.
    pub command_missing: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GitCond {
    /// In a repo's root checkout (not a linked worktree) whose branch is not
    /// the default branch.
    RootOffDefault,
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
    handlers: Vec<HandlerCfg>,
}

impl Engine {
    /// Parse a config and expand its `use = [...]` packs from the built-ins.
    pub fn from_toml(src: &str) -> Result<Engine, String> {
        Self::from_toml_in(src, None)
    }

    /// Like [`Engine::from_toml`], but a pack named `x` is read from
    /// `<pack_dir>/x.toml` first, so anyone can add packs without a PR.
    pub fn from_toml_in(src: &str, pack_dir: Option<&Path>) -> Result<Engine, String> {
        let cfg: Config = toml::from_str(src).map_err(|e| e.to_string())?;
        let mut rules = vec![];
        let mut handlers = vec![];
        for name in &cfg.packs {
            if !name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
            {
                return Err(format!("bad pack name: {name}"));
            }
            let user =
                pack_dir.and_then(|d| fs::read_to_string(d.join(format!("{name}.toml"))).ok());
            let src = match (&user, builtin_pack(name)) {
                (Some(u), _) => u.as_str(),
                (None, Some(b)) => b,
                (None, None) => return Err(format!("unknown pack: {name}")),
            };
            let p: Config = toml::from_str(src).map_err(|e| format!("pack {name}: {e}"))?;
            rules.extend(p.rule);
            handlers.extend(p.handler);
        }
        rules.extend(cfg.rule);
        handlers.extend(cfg.handler);
        Ok(Engine { rules, handlers })
    }

    /// Tools with `pre`/`post` rules: the shell glue defines wrappers for these.
    pub fn wrapped_tools(&self) -> BTreeSet<&str> {
        let rules = self
            .rules
            .iter()
            .filter(|r| matches!(r.phase, Phase::Pre | Phase::Post))
            .flat_map(|r| r.tool.iter().map(String::as_str));
        let handlers = self
            .handlers
            .iter()
            .filter(|h| {
                h.phase
                    .iter()
                    .any(|p| matches!(p, Phase::Pre | Phase::Post))
            })
            .flat_map(|h| h.tool.iter().map(String::as_str));
        rules.chain(handlers).collect()
    }

    fn applies(rule: &Rule, ev: &Event) -> bool {
        if rule.phase != ev.phase {
            return false;
        }
        if !rule.tool.is_empty() && !ev.tool.as_ref().is_some_and(|t| rule.tool.contains(t)) {
            return false;
        }
        let pos = positional(&ev.args);
        if !rule.sub.is_empty() && !pos.first().is_some_and(|a| rule.sub.contains(a)) {
            return false;
        }
        if !rule.args_start.is_empty() && !pos.starts_with(&rule.args_start) {
            return false;
        }
        let w = &rule.when;
        w.root_has.as_deref().is_none_or(|f| root_has(&ev.cwd, f))
            && w.env_set.as_deref().is_none_or(|k| ev.env.contains_key(k))
            && w.env_unset
                .as_deref()
                .is_none_or(|k| !ev.env.contains_key(k))
            && w.agent.is_none_or(|a| a == is_agent(&ev.env))
            && w.command_missing
                .as_deref()
                .is_none_or(|c| !on_path(&ev.env, c))
            && w.git.is_none_or(|g| match g {
                GitCond::RootOffDefault => {
                    git_info(&ev.cwd).is_some_and(|i| i.root_checkout && i.branch != i.default)
                }
            })
    }
}

fn root_has(cwd: &Path, file: &str) -> bool {
    cwd.ancestors().any(|d| d.join(file).is_file())
}

fn is_agent(env: &BTreeMap<String, String>) -> bool {
    AGENT_VARS.iter().any(|k| env.contains_key(*k))
}

fn on_path(env: &BTreeMap<String, String>, cmd: &str) -> bool {
    env.get("PATH")
        .is_some_and(|p| p.split(':').any(|d| Path::new(d).join(cmd).is_file()))
}

/// Arguments from the first positional one on, skipping global flags
/// (`-C <path>` and `-c <k=v>` take a value).
fn positional(args: &[String]) -> &[String] {
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-C" | "-c" => i += 2,
            a if a.starts_with('-') => i += 1,
            _ => break,
        }
    }
    args.get(i..).unwrap_or(&[])
}

struct GitInfo {
    root: PathBuf,
    branch: String,
    default: String,
    /// The repo's own checkout, not a linked worktree.
    root_checkout: bool,
}

/// Read the repo state from `.git` directly: no `git` process, so this is cheap
/// enough for a prompt. `None` outside a repo or on a detached HEAD.
fn git_info(cwd: &Path) -> Option<GitInfo> {
    let root = cwd
        .ancestors()
        .find(|d| d.join(".git").exists())?
        .to_path_buf();
    let dotgit = root.join(".git");
    let root_checkout = dotgit.is_dir();
    let gitdir = if root_checkout {
        dotgit
    } else {
        let s = fs::read_to_string(&dotgit).ok()?;
        root.join(s.strip_prefix("gitdir:")?.trim())
    };
    let head = fs::read_to_string(gitdir.join("HEAD")).ok()?;
    let branch = head.trim().strip_prefix("ref: refs/heads/")?.to_string();
    // A linked worktree keeps shared refs in the common dir.
    let common = match fs::read_to_string(gitdir.join("commondir")) {
        Ok(c) => gitdir.join(c.trim()),
        Err(_) => gitdir,
    };
    let default = fs::read_to_string(common.join("refs/remotes/origin/HEAD"))
        .ok()
        .and_then(|h| {
            h.trim()
                .strip_prefix("ref: refs/remotes/origin/")
                .map(String::from)
        })
        .or_else(|| {
            let packed = fs::read_to_string(common.join("packed-refs")).unwrap_or_default();
            ["main", "master"]
                .into_iter()
                .find(|c| {
                    common.join("refs/heads").join(c).exists()
                        || packed.contains(&format!(" refs/heads/{c}"))
                })
                .map(String::from)
        })?;
    Some(GitInfo {
        root,
        branch,
        default,
        root_checkout,
    })
}

/// Fill `{branch}`, `{default}` and `{root}` in a message.
fn fill(text: &str, cwd: &Path) -> String {
    if !text.contains('{') {
        return text.to_string();
    }
    match git_info(cwd) {
        Some(i) => text
            .replace("{branch}", &i.branch)
            .replace("{default}", &i.default)
            .replace("{root}", &i.root.to_string_lossy()),
        None => text.to_string(),
    }
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
            // "<tool> may live in..." reads right for a missing command only.
            let subject = match (&ev.phase, &ev.tool) {
                (Phase::Notfound, Some(t)) => format!("{t} "),
                _ => String::new(),
            };
            if let Some(t) = &a.hint {
                d.messages.push(Message {
                    level: Level::Hint,
                    text: format!("{subject}{}", fill(t, &ev.cwd)),
                });
            }
            if let Some(t) = &a.warn {
                d.messages.push(Message {
                    level: Level::Warn,
                    text: format!("{subject}{}", fill(t, &ev.cwd)),
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
        for h in self.handlers.iter().filter(|h| {
            (h.phase.is_empty() || h.phase.contains(&ev.phase))
                && (h.tool.is_empty() || ev.tool.as_ref().is_some_and(|t| h.tool.contains(t)))
        }) {
            match run_handler(h, ev) {
                Ok(mut out) => {
                    out.fired.push(format!("handler:{}", h.id));
                    d.merge(out);
                }
                // A broken plugin must never block the user's command.
                Err(e) => d.messages.push(Message {
                    level: Level::Warn,
                    text: format!("handler {} failed: {e}", h.id),
                }),
            }
        }
        d
    }
}

/// Run an external handler: Event JSON on stdin, Decision JSON on stdout,
/// killed after `timeout_ms`.
fn run_handler(h: &HandlerCfg, ev: &Event) -> Result<Decision, String> {
    let (prog, args) = h.command.split_first().ok_or("empty command")?;
    let mut child = Command::new(prog)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("{prog}: {e}"))?;
    let payload = serde_json::to_vec(ev).map_err(|e| e.to_string())?;
    // A handler that exits without reading is fine; ignore the broken pipe.
    let _ = child.stdin.take().ok_or("no stdin")?.write_all(&payload);
    let deadline = Instant::now() + Duration::from_millis(h.timeout_ms);
    loop {
        match child.try_wait().map_err(|e| e.to_string())? {
            Some(status) if !status.success() => return Err(format!("exited with {status}")),
            Some(_) => break,
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("timed out after {}ms", h.timeout_ms));
            }
            None => std::thread::sleep(Duration::from_millis(2)),
        }
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    serde_json::from_slice(&out.stdout).map_err(|e| format!("bad decision JSON: {e}"))
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
        let e = Engine::from_toml(include_str!("../../../examples/config.toml")).unwrap();
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

    /// A fake repo: `.git` dir on `branch`, with `main` as the default.
    fn git_repo(name: &str, branch: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("nudge-git-{name}-{}", std::process::id()));
        let g = d.join(".git");
        fs::create_dir_all(g.join("refs/heads")).unwrap();
        fs::write(g.join("HEAD"), format!("ref: refs/heads/{branch}\n")).unwrap();
        fs::write(g.join("refs/heads/main"), "").unwrap();
        d
    }

    fn git_ev(args: &[&str], cwd: &Path, env: &[(&str, &str)]) -> Event {
        let mut e = ev(Phase::Pre, Some("git"), cwd, env);
        e.args = args.iter().map(|a| a.to_string()).collect();
        e
    }

    #[test]
    fn git_pack_warns_humans_and_agents_in_off_default_root() {
        let e = Engine::from_toml(r#"use = ["git"]"#).unwrap();
        let cwd = git_repo("off", "feat/x");
        let human = e.handle(&git_ev(&["-C", ".", "commit", "-m", "x"], &cwd, &[]));
        assert!(human.proceed);
        assert!(
            human.messages[0].text.contains("'feat/x', not 'main'"),
            "{:?}",
            human.messages
        );
        let agent = e.handle(&git_ev(&["commit"], &cwd, &[("CLAUDECODE", "1")]));
        assert!(agent.proceed);
        assert!(agent.messages[0]
            .text
            .contains("do not edit or commit here"));
        // Not a gated subcommand, and on the default branch: silent.
        assert!(e
            .handle(&git_ev(&["status"], &cwd, &[]))
            .messages
            .is_empty());
        let main = git_repo("main", "main");
        assert!(e
            .handle(&git_ev(&["commit"], &main, &[]))
            .messages
            .is_empty());
    }

    #[test]
    fn git_pack_ignores_linked_worktrees_and_blocks_raw_worktree_add_for_agents() {
        let e = Engine::from_toml(r#"use = ["git"]"#).unwrap();
        let linked = std::env::temp_dir().join(format!("nudge-linked-{}", std::process::id()));
        fs::create_dir_all(&linked).unwrap();
        fs::write(linked.join(".git"), "gitdir: /nonexistent\n").unwrap();
        assert!(e
            .handle(&git_ev(&["commit"], &linked, &[]))
            .messages
            .is_empty());
        let w = e.handle(&git_ev(
            &["worktree", "add", "../x"],
            &linked,
            &[("AGENT", "1")],
        ));
        assert!(!w.proceed);
        let h = e.handle(&git_ev(&["worktree", "add", "../x"], &linked, &[]));
        assert!(h.proceed && h.messages.len() == 1);
    }

    #[test]
    fn wt_pack_warns_when_devenv_is_not_on_path() {
        let e = Engine::from_toml(r#"use = ["wt"]"#).unwrap();
        let cwd = repo("wt");
        let mut ev = ev(Phase::Pre, Some("wt"), &cwd, &[("PATH", "/nonexistent")]);
        ev.args = vec!["switch".into(), "--create".into(), "x".into()];
        assert_eq!(e.handle(&ev).fired, ["wt-switch-devenv-missing"]);
        let bin = cwd.join("bin");
        fs::create_dir_all(&bin).unwrap();
        fs::write(bin.join("devenv"), "").unwrap();
        ev.env.insert("PATH".into(), bin.to_string_lossy().into());
        assert!(e.handle(&ev).fired.is_empty());
    }

    fn script(name: &str, body: &str) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let d = std::env::temp_dir().join(format!("nudge-h-{name}-{}", std::process::id()));
        fs::create_dir_all(&d).unwrap();
        let p = d.join("h.sh");
        fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    #[test]
    fn external_handler_merges_and_a_bad_one_never_blocks() {
        let ok = script(
            "ok",
            r#"cat >/dev/null; echo '{"messages":[{"level":"hint","text":"from plugin"}],"proceed":false}'"#,
        );
        let bad = script("bad", "echo not-json");
        let cfg = format!(
            "[[handler]]\nid=\"ok\"\ncommand=[\"{}\"]\nphase=[\"pre\"]\ntool=[\"x\"]\ntimeout_ms=20000\n\
             [[handler]]\nid=\"bad\"\ncommand=[\"{}\"]\ntimeout_ms=20000\n",
            ok.display(),
            bad.display()
        );
        let e = Engine::from_toml(&cfg).unwrap();
        assert_eq!(e.wrapped_tools().into_iter().collect::<Vec<_>>(), ["x"]);
        let d = e.handle(&ev(Phase::Pre, Some("x"), Path::new("/"), &[]));
        assert!(
            !d.proceed,
            "ok handler block should apply: {:?}",
            d.messages
        );
        let texts: Vec<_> = d.messages.iter().map(|m| m.text.as_str()).collect();
        assert!(texts.contains(&"from plugin"));
        assert!(texts
            .iter()
            .any(|t| t.contains("handler bad failed: bad decision JSON")));
        assert!(d.fired.contains(&"handler:ok".to_string()));
    }

    #[test]
    fn a_slow_handler_is_killed_at_its_timeout() {
        let slow = script("slow", "sleep 30");
        let cfg = format!(
            "[[handler]]\nid=\"slow\"\ncommand=[\"{}\"]\ntimeout_ms=100\n",
            slow.display()
        );
        let e = Engine::from_toml(&cfg).unwrap();
        let t = Instant::now();
        let d = e.handle(&ev(Phase::Pre, Some("x"), Path::new("/"), &[]));
        assert!(t.elapsed() < Duration::from_secs(10), "not killed");
        assert!(d.proceed);
        assert!(d.messages[0]
            .text
            .contains("handler slow failed: timed out after 100ms"));
    }

    #[test]
    fn user_pack_dir_wins_and_names_are_checked() {
        let dir = std::env::temp_dir().join(format!("nudge-packs-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("mine.toml"),
            "[[rule]]\nid=\"m\"\ntool=[\"t\"]\nphase=\"pre\"\ndo={hint=\"mine\"}\n",
        )
        .unwrap();
        let e = Engine::from_toml_in(r#"use = ["mine"]"#, Some(&dir)).unwrap();
        assert_eq!(
            e.handle(&ev(Phase::Pre, Some("t"), Path::new("/"), &[]))
                .fired,
            ["m"]
        );
        assert!(Engine::from_toml_in(r#"use = ["../etc"]"#, Some(&dir)).is_err());
        assert!(Engine::from_toml_in(r#"use = ["missing"]"#, Some(&dir)).is_err());
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
        assert_eq!(d.messages[0].text, "careful");
        assert!(Engine::from_toml(r#"use = ["nope"]"#).is_err());
    }
}
