use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);

    match args.next().as_deref() {
        Some("--version" | "-V") | None => {
            println!("laudit {}", laudit_core::version());
            ExitCode::SUCCESS
        }
        Some("serve") => {
            report_prompt_install();
            match laudit_core::rpc::run() {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("laudit serve: {error}");
                    ExitCode::FAILURE
                }
            }
        }
        Some("prompts") => run_prompts(args.collect()),
        Some(other) => {
            eprintln!("laudit: unknown argument `{other}`");
            eprintln!(
                "usage: laudit [--version] | laudit serve | laudit prompts [<name>] [--list] [--all]"
            );
            ExitCode::from(2)
        }
    }
}

fn report_prompt_install() {
    match laudit_core::prompts::ensure_installed() {
        Ok(report) => {
            for warning in report.warnings {
                eprintln!("laudit: warning: {warning}");
            }
        }
        Err(error) => eprintln!("laudit: warning: could not install prompts: {error}"),
    }
}

enum PromptMode {
    ListAndPrint,
    List,
    Print,
}

fn run_prompts(args: Vec<String>) -> ExitCode {
    let mut mode = PromptMode::ListAndPrint;
    let mut name: Option<String> = None;

    for arg in args {
        match arg.as_str() {
            "-h" | "--help" => {
                print_prompts_usage();
                return ExitCode::SUCCESS;
            }
            "--list" => mode = PromptMode::List,
            "--all" | "--print" => mode = PromptMode::Print,
            other if other.starts_with('-') => {
                eprintln!("laudit prompts: unknown argument `{other}`");
                print_prompts_usage();
                return ExitCode::from(2);
            }
            other => {
                if name.is_some() {
                    eprintln!("laudit prompts: expected at most one prompt name");
                    return ExitCode::from(2);
                }
                name = Some(other.to_owned());
            }
        }
    }

    match laudit_core::prompts::ensure_installed() {
        Ok(report) => {
            for warning in report.warnings {
                eprintln!("laudit: warning: {warning}");
            }
        }
        Err(error) => {
            eprintln!("laudit prompts: {error}");
            return ExitCode::FAILURE;
        }
    }

    if let Some(name) = name {
        return print_prompt(&name);
    }

    match mode {
        PromptMode::List => match list_prompts() {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("laudit prompts: {error}");
                ExitCode::FAILURE
            }
        },
        PromptMode::Print => print_all_prompts(),
        PromptMode::ListAndPrint => {
            if let Err(error) = list_prompts() {
                eprintln!("laudit prompts: {error}");
                return ExitCode::FAILURE;
            }
            println!();
            print_all_prompts()
        }
    }
}

fn list_prompts() -> Result<(), laudit_core::prompts::PromptError> {
    for prompt in laudit_core::prompts::list()? {
        let status = if !prompt.installed {
            "not installed"
        } else if prompt.outdated {
            "update available"
        } else {
            "installed"
        };
        println!(
            "{:<14} v{:<3} {:<16} {}",
            prompt.name,
            prompt.version,
            status,
            prompt.path.display()
        );
    }
    Ok(())
}

fn print_prompt(name: &str) -> ExitCode {
    match laudit_core::prompts::load(name) {
        Ok(prompt) => {
            print!("{}", prompt.contents);
            if !prompt.contents.ends_with('\n') {
                println!();
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("laudit prompts: {error}");
            ExitCode::FAILURE
        }
    }
}

fn print_all_prompts() -> ExitCode {
    let names: Vec<&str> = laudit_core::prompts::embedded_names().collect();
    for (index, name) in names.iter().enumerate() {
        if index > 0 {
            println!();
        }
        println!("===== {name} =====");
        match laudit_core::prompts::load(name) {
            Ok(prompt) => {
                print!("{}", prompt.contents);
                if !prompt.contents.ends_with('\n') {
                    println!();
                }
            }
            Err(error) => {
                eprintln!("laudit prompts: {error}");
                return ExitCode::FAILURE;
            }
        }
    }
    ExitCode::SUCCESS
}

fn print_prompts_usage() {
    println!("usage: laudit prompts [<name>] [--list] [--all]");
    println!();
    println!("List installed prompts and print their contents.");
    println!("  <name>   print one prompt");
    println!("  --list   list prompts without printing their contents");
    println!("  --all    print every prompt without the list");
}
