use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);

    match args.next().as_deref() {
        Some("--version" | "-V") | None => {
            println!("laudit {}", laudit_core::version());
            ExitCode::SUCCESS
        }
        Some("serve") => match laudit_core::rpc::run() {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("laudit serve: {error}");
                ExitCode::FAILURE
            }
        },
        Some(other) => {
            eprintln!("laudit: unknown argument `{other}`");
            eprintln!("usage: laudit [--version] | laudit serve");
            ExitCode::from(2)
        }
    }
}
