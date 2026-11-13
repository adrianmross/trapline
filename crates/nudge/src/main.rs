use nudge_engine::{builtin_pack, BUILTIN_PACKS};
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        ["--version"] | ["-V"] => {
            println!("nudge {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        ["packs"] => {
            BUILTIN_PACKS.iter().for_each(|p| println!("{p}"));
            ExitCode::SUCCESS
        }
        ["show", name] => match builtin_pack(name) {
            Some(src) => {
                print!("{src}");
                ExitCode::SUCCESS
            }
            None => {
                eprintln!("nudge: unknown pack: {name}");
                ExitCode::from(2)
            }
        },
        _ => {
            eprintln!("usage: nudge packs | nudge show <pack>");
            ExitCode::from(2)
        }
    }
}
