use std::fs;
use std::path::PathBuf;
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
