use clap::{Parser, Subcommand, ValueEnum};
use nudge_engine::Engine;
use std::path::PathBuf;
use std::process::ExitCode;
use trapline_core::{Decision, Event, Handler, Level, Phase};

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
    Hook { shell: String },
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
enum PhaseArg {
    Pre,
    Post,
    Notfound,
    Chpwd,
    Prompt,
}

#[derive(Clone, Copy, ValueEnum)]
enum Format {
    /// stdout: argv prefix, one per line. stderr: messages. exit 1 = block.
    Plain,
    Json,
    /// `export K='V'` / `unset K` lines, for prompt markers.
    Shell,
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
    Engine::from_toml(&src)
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
        Cmd::Hook { shell } => {
            if shell != "zsh" {
                return Err(format!("unsupported shell: {shell} (zsh only for now)"));
            }
            print!("{ZSH_GLUE}");
            for tool in engine()?.wrapped_tools() {
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
