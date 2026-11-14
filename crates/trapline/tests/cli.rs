use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn fixture(name: &str, config: &str) -> (PathBuf, PathBuf) {
    let d = std::env::temp_dir().join(format!("trapline-{name}-{}", std::process::id()));
    fs::create_dir_all(&d).unwrap();
    fs::write(d.join("devenv.nix"), "").unwrap();
    let cfg = d.join("config.toml");
    fs::write(&cfg, config).unwrap();
    (d, cfg)
}

fn trapline(cfg: &PathBuf, cwd: &PathBuf, args: &[&str], env: &[(&str, &str)]) -> Output {
    let mut c = Command::new(env!("CARGO_BIN_EXE_trapline"));
    c.env_remove("DEVENV_ROOT")
        .env("TRAPLINE_CONFIG", cfg)
        .env("XDG_STATE_HOME", cwd.join("state"))
        .args(args)
        .arg("--cwd")
        .arg(cwd);
    for (k, v) in env {
        c.env(k, v);
    }
    c.output().unwrap()
}

#[test]
fn pre_prints_prefix_until_entered() {
    let (d, cfg) = fixture("pre", include_str!("../../../examples/config.toml"));
    let out = trapline(&cfg, &d, &["check", "pre", "nvim"], &[]);
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).starts_with("devenv\nshell\n"));
    let inside = trapline(
        &cfg,
        &d,
        &["check", "pre", "nvim"],
        &[("DEVENV_ROOT", "/x")],
    );
    assert!(inside.stdout.is_empty());
}

#[test]
fn prompt_shell_format_exports_and_unsets() {
    let (d, cfg) = fixture("prompt", r#"use = ["devenv"]"#);
    let out = trapline(&cfg, &d, &["check", "prompt", "--format", "shell"], &[]);
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(
        s.contains("export DEVENV_OFF='⬡ off'") && s.contains("unset DEVENV_ON"),
        "{s}"
    );
}

#[test]
fn block_exits_one_and_hook_defines_wrappers() {
    let cfg_src = "[[rule]]\nid=\"x\"\ntool=[\"rm\"]\nphase=\"pre\"\ndo={block=true}\n";
    let (d, cfg) = fixture("block", cfg_src);
    assert_eq!(
        trapline(&cfg, &d, &["check", "pre", "rm"], &[])
            .status
            .code(),
        Some(1)
    );
    let hook = Command::new(env!("CARGO_BIN_EXE_trapline"))
        .env("TRAPLINE_CONFIG", &cfg)
        .args(["hook", "zsh"])
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&hook.stdout).contains("rm() { _trapline_run rm"));
}

#[test]
fn hook_keeps_existing_functions_and_rejects_odd_tool_names() {
    let run = |tool: &str| {
        let src =
            format!("[[rule]]\nid=\"x\"\ntool=[\"{tool}\"]\nphase=\"pre\"\ndo={{hint=\"h\"}}\n");
        let (_, cfg) = fixture("names", &src);
        Command::new(env!("CARGO_BIN_EXE_trapline"))
            .env("TRAPLINE_CONFIG", &cfg)
            .args(["hook", "zsh"])
            .output()
            .unwrap()
    };
    let ok = run("foo");
    let out = String::from_utf8_lossy(&ok.stdout);
    assert!(out.contains("functions -c foo _trapline_orig_foo"), "{out}");
    assert_eq!(run("a;b").status.code(), Some(2));
}

fn agent_claude(cfg: &PathBuf, hook: &str) -> (Option<i32>, String) {
    agent_call(cfg, hook, "claude", None)
}

fn agent_call(
    cfg: &PathBuf,
    hook: &str,
    name: &str,
    devenv: Option<&Path>,
) -> (Option<i32>, String) {
    use std::io::Write;
    use std::process::Stdio;
    let mut command = Command::new(env!("CARGO_BIN_EXE_trapline"));
    command
        .env("TRAPLINE_CONFIG", cfg)
        .env_remove("AGENT")
        .env_remove("CLAUDECODE")
        .env_remove("DEVENV_ROOT")
        .args(["agent", name])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped());
    if let Some(path) = devenv {
        command.env("DEVENV_ROOT", path);
    }
    let mut c = command.spawn().unwrap();
    c.stdin.take().unwrap().write_all(hook.as_bytes()).unwrap();
    let out = c.wait_with_output().unwrap();
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
    )
}

#[test]
fn agent_codex_handles_rtk_and_unified_exec_without_approving_commands() {
    let (d, cfg) = fixture("codex-pre", r#"use = ["git", "wt"]"#);
    for (tool, field) in [
        ("Bash", "command"),
        ("exec_command", "cmd"),
        ("functions.exec_command", "cmd"),
    ] {
        let hook = serde_json::json!({"hook_event_name":"PreToolUse", "tool_name":tool,
            "cwd": d, "tool_input":{field:"rtk proxy git worktree add ../x"}})
        .to_string();
        let (code, output) = agent_call(&cfg, &hook, "codex", None);
        assert_eq!(code, Some(0));
        let value: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(value["hookSpecificOutput"]["permissionDecision"], "deny");
        assert!(value["hookSpecificOutput"].get("updatedInput").is_none());
    }
    let hook = serde_json::json!({"hook_event_name":"PreToolUse", "tool_name":"Bash",
        "cwd": d, "tool_input":{"command":"rtk wt remove example"}})
    .to_string();
    let (_, output) = agent_call(&cfg, &hook, "codex", None);
    let value: serde_json::Value = serde_json::from_str(&output).unwrap();
    assert!(value["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap()
        .contains("unpushed"));
    assert!(value["hookSpecificOutput"]
        .get("permissionDecision")
        .is_none());
}

#[test]
fn agent_codex_reports_actual_activation_and_never_rewrites_wrappers() {
    let (d, cfg) = fixture("codex-session", r#"use = ["devenv-agent"]"#);
    let hook = serde_json::json!({"hook_event_name":"SessionStart", "cwd": d}).to_string();
    let (_, outside) = agent_call(&cfg, &hook, "codex", None);
    assert!(outside.contains("has not entered"));
    let (_, inside) = agent_call(&cfg, &hook, "codex", Some(&d));
    assert!(inside.contains("Verify it matches"));
    assert!(!inside.contains("has not entered"));
    let (d, cfg) = fixture(
        "codex-wrapper",
        include_str!("../../../examples/config.toml"),
    );
    let hook = serde_json::json!({"hook_event_name":"PreToolUse", "tool_name":"exec_command",
        "cwd":"/", "tool_input":{"cmd":"rtk proxy nvim README.md", "workdir":d}})
    .to_string();
    let (_, output) = agent_call(&cfg, &hook, "codex", None);
    assert!(output.contains("leaves the command unchanged"));
    assert!(!output.contains("updatedInput"));
    assert!(!output.contains("permissionDecision"));
    assert_eq!(
        agent_call(&cfg, "not json", "codex", None),
        (Some(0), String::new())
    );
    let hook =
        serde_json::json!({"hook_event_name":"PreToolUse", "tool_name":"apply_patch", "cwd":d,
        "tool_input":{"command":"git worktree add /tmp/ignored"}})
        .to_string();
    assert_eq!(
        agent_call(&cfg, &hook, "codex", None),
        (Some(0), String::new())
    );
}

#[test]
fn agent_claude_denies_raw_worktree_add_inside_a_compound_command() {
    let (d, cfg) = fixture("agent-deny", r#"use = ["git"]"#);
    let hook = format!(
        r#"{{"hook_event_name":"PreToolUse","tool_name":"Bash","cwd":"{}","tool_input":{{"command":"cd /tmp && git worktree add ../x"}}}}"#,
        d.display()
    );
    let (code, out) = agent_claude(&cfg, &hook);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(code, Some(0));
    assert_eq!(v["hookSpecificOutput"]["permissionDecision"], "deny");
    assert!(v["hookSpecificOutput"]["permissionDecisionReason"]
        .as_str()
        .unwrap()
        .contains("wt switch"));
}

#[test]
fn agent_claude_session_start_adds_devenv_context_and_ignores_other_tools() {
    let (d, cfg) = fixture("agent-ctx", r#"use = ["devenv-agent"]"#);
    let start = format!(
        r#"{{"hook_event_name":"SessionStart","cwd":"{}"}}"#,
        d.display()
    );
    let (_, out) = agent_claude(&cfg, &start);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert!(v["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap()
        .contains("devenv"));
    let read = format!(
        r#"{{"hook_event_name":"PreToolUse","tool_name":"Read","cwd":"{}","tool_input":{{}}}}"#,
        d.display()
    );
    assert_eq!(agent_claude(&cfg, &read), (Some(0), String::new()));
    assert_eq!(agent_claude(&cfg, "not json"), (Some(0), String::new()));
}

#[test]
fn eval_reads_an_event_and_external_agent_adapters_dispatch_from_path() {
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;
    use std::process::Stdio;
    let (d, cfg) = fixture("eval", r#"use = ["git"]"#);
    let ev = r#"{"phase":"pre","tool":"git","args":["worktree","add","x"],"cwd":"/","env":{"AGENT":"1"}}"#;
    let mut c = Command::new(env!("CARGO_BIN_EXE_trapline"))
        .env("TRAPLINE_CONFIG", &cfg)
        .arg("eval")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    c.stdin.take().unwrap().write_all(ev.as_bytes()).unwrap();
    let out = c.wait_with_output().unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["proceed"], false);

    let bin = d.join("trapline-agent-demo");
    fs::write(&bin, "#!/bin/sh\necho adapter-ran $1\nexit 7\n").unwrap();
    fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_trapline"))
        .env("PATH", format!("{}:/usr/bin:/bin", d.display()))
        .args(["agent", "demo", "--", "arg1"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(7));
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "adapter-ran arg1"
    );
}

fn have(cmd: &str) -> bool {
    Command::new("sh")
        .args(["-c", &format!("command -v {cmd}")])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Run `hook zsh` (compiled or not) in a zsh and print the prompt markers and
/// the not-found hint for one scenario.
fn zsh_scenario(cfg: &Path, cwd: &Path, compile: bool, entered: bool) -> String {
    let hook = format!(
        "eval \"$({} hook zsh {})\"",
        env!("CARGO_BIN_EXE_trapline"),
        if compile { "" } else { "--no-compile" }
    );
    let script = format!(
        "{hook}; cd {cwd}; {export}; PATH=\"$PATH\"; _trapline_precmd; \
         print \"ON=[$DEVENV_ON] OFF=[$DEVENV_OFF]\"; _trapline_notfound_hint nosuchtool",
        cwd = cwd.display(),
        export = if entered {
            "export DEVENV_ROOT=/x"
        } else {
            "unset DEVENV_ROOT"
        },
    );
    let out = Command::new("zsh")
        .args(["-f", "-c", &script])
        .env("TRAPLINE_CONFIG", cfg)
        .env(
            "PATH",
            format!(
                "{}:{}",
                std::path::Path::new(env!("CARGO_BIN_EXE_trapline"))
                    .parent()
                    .unwrap()
                    .display(),
                std::env::var("PATH").unwrap()
            ),
        )
        .output()
        .unwrap();
    String::from_utf8_lossy(&[out.stdout, out.stderr].concat()).into_owned()
}

#[test]
fn compiled_zsh_matches_the_binary_in_every_scenario() {
    if !have("zsh") {
        return;
    }
    let (d, cfg) = fixture("compile", r#"use = ["devenv"]"#);
    let outside = std::env::temp_dir().join(format!("trapline-outside-{}", std::process::id()));
    fs::create_dir_all(&outside).unwrap();
    for (cwd, entered) in [(&d, false), (&d, true), (&outside, false)] {
        let compiled = zsh_scenario(&cfg, cwd, true, entered);
        let binary = zsh_scenario(&cfg, cwd, false, entered);
        assert_eq!(compiled, binary, "cwd={} entered={entered}", cwd.display());
    }
    assert!(zsh_scenario(&cfg, &d, true, false).contains("OFF=[⬡ off]"));
}

fn git_repo(name: &str) -> PathBuf {
    let d = std::env::temp_dir()
        .canonicalize()
        .unwrap()
        .join(format!("trapline-sbx-{name}-{}", std::process::id()));
    fs::create_dir_all(&d).unwrap();
    for args in [
        vec!["init", "-q", "-b", "main"],
        vec![
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "i",
        ],
    ] {
        assert!(Command::new("git")
            .args(&args)
            .current_dir(&d)
            .status()
            .unwrap()
            .success());
    }
    d
}

#[test]
fn sandbox_profile_denies_the_root_checkout_and_really_enforces_on_macos() {
    if !have("git") {
        return;
    }
    let repo = git_repo("enforce");
    let (_, cfg) = fixture("sbx", r#"use = ["git"]"#);
    let out = Command::new(env!("CARGO_BIN_EXE_trapline"))
        .env("TRAPLINE_CONFIG", &cfg)
        .args(["sandbox", "seatbelt", "--cwd"])
        .arg(&repo)
        .output()
        .unwrap();
    let profile = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        profile.contains(&format!(
            "(deny file-write* (subpath \"{}\"))",
            repo.display()
        )),
        "{profile}"
    );
    assert!(profile.contains(&format!(
        "(allow file-write* (subpath \"{}/.git\"))",
        repo.display()
    )));

    // Enforcement is macOS-only, and only if sandbox-exec works here (it cannot
    // nest inside another sandbox).
    if !cfg!(target_os = "macos") || !have("sandbox-exec") {
        return;
    }
    let works = Command::new("sandbox-exec")
        .args(["-p", "(version 1)(allow default)", "/usr/bin/true"])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !works {
        return;
    }
    let run = |path: &str| {
        Command::new("sandbox-exec")
            .args(["-p", &profile, "/usr/bin/touch", path])
            .status()
            .unwrap()
            .success()
    };
    assert!(
        !run(&format!("{}/new.txt", repo.display())),
        "root checkout must be read-only"
    );
    assert!(
        run(&format!("{}/.git/new.txt", repo.display())),
        ".git must stay writable"
    );
}

#[test]
fn run_points_the_command_at_the_named_profile() {
    let (d, _) = fixture("run", "");
    let out = Command::new(env!("CARGO_BIN_EXE_trapline"))
        .env_remove("TRAPLINE_CONFIG")
        .env("XDG_CONFIG_HOME", &d)
        .args(["run", "--profile", "agent", "--no-sandbox", "--"])
        .args(["sh", "-c", "printf %s \"$TRAPLINE_CONFIG\""])
        .output()
        .unwrap();
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        d.join("trapline/agent.toml").to_string_lossy()
    );
    let bad = Command::new(env!("CARGO_BIN_EXE_trapline"))
        .args(["run", "--profile", "../x", "--", "true"])
        .output()
        .unwrap();
    assert!(!bad.status.success());
}

#[test]
fn explain_says_why_and_log_remembers_what_fired() {
    let (d, cfg) = fixture("explain", include_str!("../../../examples/config.toml"));
    let out = trapline(&cfg, &d, &["explain", "pre", "nvim"], &[]);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("fired"), "{text}");
    let inside = trapline(
        &cfg,
        &d,
        &["explain", "pre", "nvim"],
        &[("DEVENV_ROOT", "/x")],
    );
    assert!(String::from_utf8_lossy(&inside.stdout).contains("skipped"));

    trapline(&cfg, &d, &["check", "pre", "nvim"], &[]);
    let state = d.join("state");
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_trapline"))
            .env("XDG_STATE_HOME", &state)
            .args(args)
            .output()
            .unwrap()
    };
    let stats = String::from_utf8_lossy(&run(&["log", "--stats"]).stdout).into_owned();
    assert!(stats.trim_start().starts_with('1'), "{stats}");
    assert!(String::from_utf8_lossy(&run(&["log"]).stdout).contains("pre"));
}

/// Run `_trapline_pre` in zsh (compiled or via the binary) for one command and
/// report its status, prefix and messages.
fn pre_scenario(cfg: &Path, cwd: &Path, compile: bool, agent: bool, argv: &str) -> String {
    let script = format!(
        "eval \"$({bin} hook zsh {flag})\"; cd '{cwd}' || exit 9; local -a _tl_prefix; \
         _trapline_pre {argv}; print \"rc=$? prefix=(${{_tl_prefix}})\"",
        bin = env!("CARGO_BIN_EXE_trapline"),
        flag = if compile { "" } else { "--no-compile" },
        cwd = cwd.display(),
    );
    let mut c = Command::new("zsh");
    c.args(["-f", "-c", &script])
        .env("TRAPLINE_CONFIG", cfg)
        .env("XDG_STATE_HOME", cwd.join("state"))
        .env_remove("AGENT")
        .env_remove("CLAUDECODE")
        .env_remove("DEVENV_ROOT")
        .env(
            "PATH",
            format!(
                "{}:{}",
                Path::new(env!("CARGO_BIN_EXE_trapline"))
                    .parent()
                    .unwrap()
                    .display(),
                std::env::var("PATH").unwrap()
            ),
        );
    if agent {
        c.env("CLAUDECODE", "1");
    }
    let out = c.output().unwrap();
    String::from_utf8_lossy(&[out.stdout, out.stderr].concat()).into_owned()
}

#[test]
fn compiled_pre_matches_the_binary_across_repo_states() {
    if !have("zsh") || !have("git") {
        return;
    }
    let cfg_dir = std::env::temp_dir().join(format!("trapline-pre-{}", std::process::id()));
    fs::create_dir_all(&cfg_dir).unwrap();
    let cfg = cfg_dir.join("config.toml");
    fs::write(
        &cfg,
        r#"use = ["git", "wt"]
[[rule]]
id = "nvim"
tool = ["nvim"]
phase = "pre"
when = { root_has = "devenv.nix", env_unset = "DEVENV_ROOT" }
do = { wrap = ["devenv", "shell", "--no-tui", "--"] }
"#,
    )
    .unwrap();

    let on_main = git_repo("pre-main");
    fs::write(on_main.join("devenv.nix"), "").unwrap();
    let off_main = git_repo("pre-feat");
    let git = |d: &Path, a: &[&str]| {
        assert!(Command::new("git")
            .args(a)
            .current_dir(d)
            .status()
            .unwrap()
            .success())
    };
    git(&off_main, &["checkout", "-q", "-b", "feat/x"]);
    let linked = off_main.with_file_name(format!(
        "{}-wt",
        off_main.file_name().unwrap().to_string_lossy()
    ));
    git(&off_main, &["checkout", "-q", "main"]);
    git(
        &off_main,
        &[
            "worktree",
            "add",
            "-q",
            linked.to_str().unwrap(),
            "-b",
            "feat/y",
        ],
    );
    git(&off_main, &["checkout", "-q", "feat/x"]);
    let plain = cfg_dir.join("plain");
    fs::create_dir_all(&plain).unwrap();

    // Shapes where the zsh and Rust `.git` readers are most likely to differ.
    let detached = git_repo("pre-detached");
    git(&detached, &["checkout", "-q", "--detach"]);
    let packed = git_repo("pre-packed");
    git(&packed, &["checkout", "-q", "-b", "feat/z"]);
    git(&packed, &["pack-refs", "--all"]);
    let spaced = git_repo("pre-sp ace");
    git(&spaced, &["checkout", "-q", "-b", "feat/s"]);

    for cwd in [
        &on_main, &off_main, &linked, &plain, &detached, &packed, &spaced,
    ] {
        for agent in [false, true] {
            for argv in [
                "git commit -m x",
                "git -C /tmp commit -m x",
                "git status",
                "git worktree add ../x",
                "wt switch --create y",
                "wt remove y",
                "nvim file",
                "ls",
            ] {
                let a = pre_scenario(&cfg, cwd, true, agent, argv);
                let b = pre_scenario(&cfg, cwd, false, agent, argv);
                assert_eq!(a, b, "cwd={} agent={agent} argv={argv}", cwd.display());
            }
        }
    }
    // The comparison must have exercised a firing rule, not only silence.
    let hit = pre_scenario(&cfg, &off_main, true, true, "git commit -m x");
    assert!(
        hit.contains("root checkout is on 'feat/x', not 'main'"),
        "{hit}"
    );
    assert!(pre_scenario(&cfg, &on_main, true, true, "git worktree add x").contains("rc=1"));
}

#[test]
fn wrapper_still_runs_the_tool_when_the_helpers_are_gone() {
    if !have("zsh") {
        return;
    }
    let (d, cfg) = fixture("wrapfallback", "use = [\"devenv\"]\n[[rule]]\nid = \"w\"\ntool = [\"echo\"]\nphase = \"pre\"\ndo = { hint = \"x\" }\n");
    let script = format!(
        "eval \"$({} hook zsh)\"; unfunction _trapline_run _trapline_pre _trapline_post; echo hi",
        env!("CARGO_BIN_EXE_trapline")
    );
    let out = Command::new("zsh")
        .args(["-f", "-c", &script])
        .env("TRAPLINE_CONFIG", &cfg)
        .current_dir(&d)
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout), "hi\n");
    assert!(
        out.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}
