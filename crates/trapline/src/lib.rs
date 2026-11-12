use clap::{Parser, Subcommand, ValueEnum};
use nudge_engine::Engine;
use std::path::PathBuf;
use std::process::ExitCode;
use trapline_core::{Decision, Event, Handler, Level, Phase};

pub mod agent;

const ZSH_GLUE: &str = include_str!("glue.zsh");

#[derive(Parser)]
#[command(name = "trapline", version, about = "Events at the command boundary")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Print shell glue to eval from your rc file.
    Hook {
        shell: String,
        /// Keep calling the binary for prompt/chpwd/notfound instead of compiling
        /// those rules to zsh.
        #[arg(long)]
        no_compile: bool,
    },
    /// Print a sandbox profile that enforces the `sandbox` rules for a directory.
    /// macOS: `sandbox-exec -p "$(trapline sandbox seatbelt)" claude`.
    Sandbox {
        #[arg(value_enum)]
        kind: SandboxKind,
        #[arg(long)]
        cwd: Option<PathBuf>,
    },
    /// Run a command with a rule profile and, where available, its sandbox:
    /// `trapline run --profile agent -- claude`.
    Run {
        /// Config name under the trapline config dir (`agent` = agent.toml).
        #[arg(long)]
        profile: Option<String>,
        /// Skip the sandbox; only set the profile for the command's hooks.
        #[arg(long)]
        no_sandbox: bool,
        #[arg(last = true, required = true)]
        cmd: Vec<String>,
    },
    /// Answer an agent's hook call (JSON on stdin) with the same rules. `claude`
    /// is built in; any other name runs `trapline-agent-<name>` from PATH.
    Agent {
        name: String,
        /// Extra arguments for an external adapter.
        #[arg(last = true)]
        args: Vec<String>,
    },
    /// Evaluate one Event (JSON on stdin) and print the Decision as JSON. This
    /// is the API an adapter written in any language calls.
    Eval,
    /// Evaluate the rules for one event.
    Check {
        #[arg(value_enum)]
        phase: PhaseArg,
        /// The tool the event is about (`pre`, `post`, `notfound`).
        tool: Option<String>,
        #[arg(long)]
        rc: Option<i32>,
        #[arg(long)]
        cwd: Option<PathBuf>,
        #[arg(long, value_enum, default_value = "plain")]
        format: Format,
        /// Arguments of the wrapped command.
        #[arg(last = true)]
        args: Vec<String>,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum SandboxKind {
    /// macOS `sandbox-exec` profile text.
    Seatbelt,
    /// Linux bubblewrap argv, one argument per line.
    Bwrap,
}

#[derive(Clone, Copy, ValueEnum)]
enum PhaseArg {
    Pre,
    Post,
    Notfound,
    Chpwd,
    Prompt,
    SessionStart,
}

#[derive(Clone, Copy, ValueEnum)]
enum Format {
    /// stdout: argv prefix, one per line. stderr: messages. exit 1 = block.
    Plain,
    Json,
    /// `export K='V'` / `unset K` lines, for prompt markers.
    Shell,
}

fn sandbox_text(kind: SandboxKind, cwd: PathBuf) -> Result<String, String> {
    let mut out = String::new();
    let ev = Event {
        phase: Phase::Sandbox,
        tool: None,
        args: vec![],
        cwd,
        env: std::env::vars().collect(),
        rc: None,
    };
    let d = engine()?.handle(&ev);
    // Profiles match real paths, and /tmp is really /private/tmp. A path
    // that does not exist yet (a fresh repo has no index) is resolved
    // through its parent.
    let real = |paths: &[String]| -> Result<Vec<String>, String> {
        paths
            .iter()
            .map(|p| {
                let path = std::path::Path::new(p);
                let c = std::fs::canonicalize(path).or_else(|e| {
                    match (path.parent(), path.file_name()) {
                        (Some(parent), Some(name)) => {
                            std::fs::canonicalize(parent).map(|d| d.join(name))
                        }
                        _ => Err(e),
                    }
                });
                let c = c
                    .map_err(|e| format!("{p}: {e}"))?
                    .to_string_lossy()
                    .into_owned();
                if c.contains(['"', '\n']) {
                    return Err(format!("unsupported path: {c:?}"));
                }
                Ok(c)
            })
            .collect()
    };
    let (deny, allow, lock) = (
        real(&d.fs.deny_write)?,
        real(&d.fs.allow_write)?,
        real(&d.fs.lock_write)?,
    );
    match kind {
        SandboxKind::Seatbelt => {
            out.push_str("(version 1)\n(allow default)\n");
            for p in &deny {
                out.push_str(&format!("(deny file-write* (subpath \"{p}\"))\n"));
            }
            // Later rules win: allows after denies, locks after allows.
            for p in &allow {
                out.push_str(&format!("(allow file-write* (subpath \"{p}\"))\n"));
            }
            for p in &lock {
                out.push_str(&format!("(deny file-write* (subpath \"{p}\"))\n"));
            }
        }
        SandboxKind::Bwrap => {
            out.push_str("bwrap\n--dev-bind\n/\n/\n");
            for p in &deny {
                out.push_str(&format!("--ro-bind\n{p}\n{p}\n"));
            }
            for p in &allow {
                out.push_str(&format!("--bind\n{p}\n{p}\n"));
            }
            for p in &lock {
                out.push_str(&format!("--ro-bind\n{p}\n{p}\n"));
            }
            out.push_str("--\n");
        }
    }
    Ok(out)
}

fn launch(profile: Option<String>, no_sandbox: bool, cmd: Vec<String>) -> Result<ExitCode, String> {
    use std::os::unix::process::CommandExt;
    if let Some(name) = &profile {
        if !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err(format!("bad profile name: {name}"));
        }
        let dir = config_path()
            .and_then(|p| p.parent().map(|d| d.to_path_buf()))
            .ok_or("no config dir")?;
        // The sandbox below and the child's hooks must read the same file.
        std::env::set_var("TRAPLINE_CONFIG", dir.join(format!("{name}.toml")));
    }
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let mut argv: Vec<String> = Vec::new();
    if !no_sandbox {
        if cfg!(target_os = "macos") {
            argv.extend([
                "sandbox-exec".into(),
                "-p".into(),
                sandbox_text(SandboxKind::Seatbelt, cwd)?,
            ]);
        } else if cfg!(target_os = "linux") {
            argv.extend(
                sandbox_text(SandboxKind::Bwrap, cwd)?
                    .lines()
                    .map(String::from),
            );
            // The last line is bwrap's own `--`; cmd follows it.
        }
    }
    argv.extend(cmd);
    let err = std::process::Command::new(&argv[0]).args(&argv[1..]).exec();
    Err(format!("{}: {err}", argv[0]))
}

fn config_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("TRAPLINE_CONFIG") {
        return Some(p.into());
    }
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("trapline/config.toml"))
}

fn engine() -> Result<Engine, String> {
    let src = match config_path() {
        Some(p) => match std::fs::read_to_string(&p) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(format!("{}: {e}", p.display())),
        },
        None => String::new(),
    };
    let packs = config_path().and_then(|p| p.parent().map(|d| d.join("packs")));
    Engine::from_toml_in(&src, packs.as_deref())
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

fn print_messages(d: &Decision) {
    for m in &d.messages {
        let color = match m.level {
            Level::Hint => "90",
            Level::Warn => "33",
        };
        eprintln!("\x1b[{color}m⬡ {}\x1b[0m", m.text);
    }
}

/// Entry point shared by the `trapline` and `tl` binaries.
pub fn run_main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("trapline: {e}");
            ExitCode::from(2)
        }
    }
}

fn run(cli: Cli) -> Result<ExitCode, String> {
    match cli.cmd {
        Cmd::Hook { shell, no_compile } => {
            if shell != "zsh" {
                return Err(format!("unsupported shell: {shell} (zsh only for now)"));
            }
            print!("{ZSH_GLUE}");
            let engine = engine()?;
            if !no_compile {
                print!("{}", engine.compile_zsh());
            }
            for tool in engine.wrapped_tools() {
                // Tool names go into shell code; keep them to plain command names.
                if !tool
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "_.+-".contains(c))
                {
                    return Err(format!("cannot wrap tool with name {tool:?}"));
                }
                // Keep a function the tool already has, unless it is our own.
                println!(
                    "(( $+functions[{tool}] )) && [[ ${{functions[{tool}]}} != *_trapline_run* ]] \
                     && functions -c {tool} _trapline_orig_{tool}"
                );
                println!("{tool}() {{ _trapline_run {tool} \"$@\" }}");
            }
            Ok(ExitCode::SUCCESS)
        }
        Cmd::Sandbox { kind, cwd } => {
            let cwd = cwd.map_or_else(|| std::env::current_dir().map_err(|e| e.to_string()), Ok)?;
            print!("{}", sandbox_text(kind, cwd)?);
            Ok(ExitCode::SUCCESS)
        }
        Cmd::Run {
            profile,
            no_sandbox,
            cmd,
        } => launch(profile, no_sandbox, cmd),
        Cmd::Eval => {
            let stdin = std::io::read_to_string(std::io::stdin()).map_err(|e| e.to_string())?;
            let ev: Event = serde_json::from_str(&stdin).map_err(|e| format!("bad event: {e}"))?;
            let d = engine()?.handle(&ev);
            println!("{}", serde_json::to_string(&d).map_err(|e| e.to_string())?);
            Ok(ExitCode::SUCCESS)
        }
        Cmd::Agent { name, args } if name != "claude" => {
            // git-style plugin: trapline-agent-<name> on PATH owns the protocol.
            if !name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
            {
                return Err(format!("bad agent name: {name}"));
            }
            let bin = format!("trapline-agent-{name}");
            let status = std::process::Command::new(&bin)
                .args(args)
                .status()
                .map_err(|e| format!("{bin}: {e} (is it on PATH?)"))?;
            Ok(ExitCode::from(status.code().unwrap_or(1) as u8))
        }
        Cmd::Agent { .. } => {
            let stdin = std::io::read_to_string(std::io::stdin()).map_err(|e| e.to_string())?;
            if let Some(out) = agent::run_claude(&stdin, &engine()?, std::env::vars().collect()) {
                println!("{out}");
            }
            Ok(ExitCode::SUCCESS)
        }
        Cmd::Check {
            phase,
            tool,
            rc,
            cwd,
            format,
            args,
        } => {
            let ev = Event {
                phase: match phase {
                    PhaseArg::Pre => Phase::Pre,
                    PhaseArg::Post => Phase::Post,
                    PhaseArg::Notfound => Phase::Notfound,
                    PhaseArg::Chpwd => Phase::Chpwd,
                    PhaseArg::Prompt => Phase::Prompt,
                    PhaseArg::SessionStart => Phase::SessionStart,
                },
                tool,
                args,
                cwd: cwd.map_or_else(|| std::env::current_dir().map_err(|e| e.to_string()), Ok)?,
                env: std::env::vars().collect(),
                rc,
            };
            let d = engine()?.handle(&ev);
            match format {
                Format::Json => {
                    println!("{}", serde_json::to_string(&d).map_err(|e| e.to_string())?)
                }
                Format::Plain => {
                    print_messages(&d);
                    for a in &d.prefix {
                        println!("{a}");
                    }
                }
                Format::Shell => {
                    print_messages(&d);
                    for (k, v) in &d.set_env {
                        match v {
                            Some(v) => println!("export {k}={}", shell_quote(v)),
                            None => println!("unset {k}"),
                        }
                    }
                }
            }
            Ok(if d.proceed {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            })
        }
    }
}
