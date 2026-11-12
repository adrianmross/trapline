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
    use std::io::Write;
    use std::process::Stdio;
    let mut c = Command::new(env!("CARGO_BIN_EXE_trapline"))
        .env("TRAPLINE_CONFIG", cfg)
        .env_remove("AGENT")
        .env_remove("CLAUDECODE")
        .args(["agent", "claude"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    c.stdin.take().unwrap().write_all(hook.as_bytes()).unwrap();
    let out = c.wait_with_output().unwrap();
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
    )
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
