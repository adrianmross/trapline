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
    /// and `codex` are built in; other names run `trapline-agent-<name>` from PATH.
    Agent {
        name: String,
        /// Extra arguments for an external adapter.
        #[arg(last = true)]
        args: Vec<String>,
    },
    /// Evaluate one Event (JSON on stdin) and print the Decision as JSON. This
    /// is the API an adapter written in any language calls.
    Eval,
    /// Show, for each rule in the event's phase, whether it fired and why not.
    Explain {
        #[arg(value_enum)]
        phase: PhaseArg,
        tool: Option<String>,
        #[arg(long)]
        cwd: Option<PathBuf>,
        #[arg(last = true)]
        args: Vec<String>,
    },
    /// Show recent rule firings, or with `--stats` how often each rule fired.
    Log {
        #[arg(short, default_value_t = 20)]
        n: usize,
        #[arg(long)]
        stats: bool,
    },
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

fn cli_event(
    phase: PhaseArg,
    tool: Option<String>,
    args: Vec<String>,
    cwd: Option<PathBuf>,
    rc: Option<i32>,
) -> Result<Event, String> {
    Ok(Event {
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
    })
}

/// `$XDG_STATE_HOME/trapline/fired.jsonl`, one line per event that fired a rule.
fn log_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))?;
    Some(base.join("trapline/fired.jsonl"))
}

/// Best-effort: a log that cannot be written must never affect the command.
/// Rules compiled into the shell glue do not pass through here.
fn record(ev: &Event, d: &Decision) {
    use std::io::Write;
    if d.fired.is_empty() {
        return;
    }
    let Some(p) = log_path() else { return };
    let _ = std::fs::create_dir_all(p.parent().unwrap());
    // ponytail: unbounded append; add rotation if the file ever gets large.
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |t| t.as_secs());
    let line = serde_json::json!({
        "ts": ts, "phase": ev.phase, "tool": ev.tool, "cwd": ev.cwd,
        "fired": d.fired, "proceed": d.proceed,
    });
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(p)
    {
        let _ = writeln!(f, "{line}");
    }
}

/// Records every decision it hands back.
struct Logged(Engine);

impl Handler for Logged {
    fn handle(&self, ev: &Event) -> Decision {
        let d = self.0.handle(ev);
        record(ev, &d);
        d
    }
}

fn show_log(n: usize, stats: bool) -> Result<ExitCode, String> {
    let text = log_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .unwrap_or_default();
    let rows: Vec<serde_json::Value> = text
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    if stats {
        let mut counts = std::collections::BTreeMap::<String, usize>::new();
        for r in &rows {
            for id in r["fired"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|v| v.as_str())
            {
                *counts.entry(id.to_string()).or_default() += 1;
            }
        }
        let mut v: Vec<_> = counts.into_iter().collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        v.iter().for_each(|(id, c)| println!("{c:>6}  {id}"));
    } else {
        for r in rows.iter().rev().take(n).rev() {
            println!(
                "{}  {:<12} {:<10} {}{}",
                r["ts"],
                r["phase"].as_str().unwrap_or(""),
                r["tool"].as_str().unwrap_or("-"),
                r["fired"],
                if r["proceed"] == false {
                    "  BLOCKED"
                } else {
                    ""
                }
            );
        }
    }
    Ok(ExitCode::SUCCESS)
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
                // A shell snapshot (an agent's Bash tool, say) can keep this function
                // but drop the `_`-prefixed helpers; then run the tool plainly.
                println!(
                    "{tool}() {{ if (( $+functions[_trapline_run] )); then _trapline_run {tool} \"$@\"; \
                     else command {tool} \"$@\"; fi }}"
                );
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
            let d = Logged(engine()?).handle(&ev);
            println!("{}", serde_json::to_string(&d).map_err(|e| e.to_string())?);
            Ok(ExitCode::SUCCESS)
        }
        Cmd::Agent { name, args } if name != "claude" && name != "codex" => {
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
        Cmd::Agent { name, .. } => {
            let stdin = std::io::read_to_string(std::io::stdin()).map_err(|e| e.to_string())?;
            let run = if name == "codex" {
                agent::run_codex
            } else {
                agent::run_claude
            };
            if let Some(out) = run(&stdin, &Logged(engine()?), std::env::vars().collect()) {
                println!("{out}");
            }
            Ok(ExitCode::SUCCESS)
        }
        Cmd::Explain {
            phase,
            tool,
            cwd,
            args,
        } => {
            let ev = cli_event(phase, tool, args, cwd, None)?;
            let rows = engine()?.explain(&ev);
            if rows.is_empty() {
                println!("no rules in this phase");
            }
            for (id, why) in rows {
                match why {
                    None => println!("fired    {id}"),
                    Some(w) => println!("skipped  {id}: {w}"),
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        Cmd::Log { n, stats } => show_log(n, stats),
        Cmd::Check {
            phase,
            tool,
            rc,
            cwd,
            format,
            args,
        } => {
            let ev = cli_event(phase, tool, args, cwd, rc)?;
            let d = engine()?.handle(&ev);
            record(&ev, &d);
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
