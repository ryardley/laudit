use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut repo = PathBuf::from(".");
    let mut offline = false;
    let mut args = std::env::args().skip(1);

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--repo" => {
                let Some(path) = args.next() else {
                    eprintln!("laudit-tui: --repo requires a path");
                    return ExitCode::FAILURE;
                };
                repo = PathBuf::from(path);
            }
            "--offline" => offline = true,
            "--version" | "-V" => {
                println!("laudit-tui {}", env!("CARGO_PKG_VERSION"));
                return ExitCode::SUCCESS;
            }
            "--help" | "-h" => {
                println!("laudit-tui [--repo <path>] [--offline]");
                return ExitCode::SUCCESS;
            }
            other => {
                eprintln!("laudit-tui: unexpected argument `{other}`");
                return ExitCode::FAILURE;
            }
        }
    }

    match laudit_tui::run(repo, offline) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("laudit-tui: {error}");
            ExitCode::FAILURE
        }
    }
}
