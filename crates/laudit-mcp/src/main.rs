use std::process::ExitCode;

fn main() -> ExitCode {
    match laudit_mcp::run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("laudit mcp: {error}");
            ExitCode::FAILURE
        }
    }
}
