use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);

    match args.next().as_deref() {
        Some("--version" | "-V") | None => {
            println!("laudit {}", laudit_core::version());
            ExitCode::SUCCESS
        }
        Some(other) => {
            eprintln!("laudit: unknown argument `{other}`");
            eprintln!("usage: laudit [--version]");
            ExitCode::from(2)
        }
    }
}
