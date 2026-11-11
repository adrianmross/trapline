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
