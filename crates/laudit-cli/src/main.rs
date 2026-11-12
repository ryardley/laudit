use std::io::IsTerminal;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{ArgGroup, Args, CommandFactory, Parser, Subcommand, ValueEnum};

#[derive(Debug, Parser)]
#[command(
    name = "laudit",
    version,
    about = "Guided codebase tours",
    disable_help_subcommand = true
)]
struct Cli {
    /// Repository root to operate on
    #[arg(long, global = true, value_name = "PATH")]
    repo: Option<PathBuf>,

    /// Run recon only; no LLM calls
    #[arg(long, global = true)]
    offline: bool,

    /// Emit machine-readable JSON
    #[arg(long, global = true)]
    json: bool,

    /// Color output
    #[arg(long, global = true, value_enum, default_value_t = ColorChoice::Auto)]
    color: ColorChoice,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum ColorChoice {
    Auto,
    Always,
    Never,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run recon (+ comprehension/plan unless --offline)
    Build(BuildArgs),
    /// Start the JSON-RPC 2.0 stdio server
    Serve,
    /// Run or print a tour
    Tour(TourArgs),
    /// Print a grounded explanation for one symbol
    Explain {
        /// Symbol to explain
        symbol: String,
    },
    /// Render a tour to markdown or HTML
    Export(ExportArgs),
    /// Grounded Q&A over the codebase model
    Ask {
        /// Question to answer; starts a REPL when omitted
        question: Option<String>,
    },
    /// Start the MCP stdio server
    Mcp,
    /// Print the tour outline
    Outline,
    /// Report model/tour presence and counts
    Status {
        /// Tour id to report; defaults to the most recent tour
        #[arg(long, value_name = "ID")]
        tour: Option<String>,
    },
    /// List or print installed prompts
    Prompts(PromptsArgs),
    /// Remove generated artifacts
    Clear,
}

#[derive(Debug, Args)]
struct BuildArgs {
    /// Exit non-zero if artifacts are stale
    #[arg(long)]
    check: bool,
    /// Suppress progress output
    #[arg(long)]
    quiet: bool,
    /// Include test files in the tour plan
    #[arg(long)]
    include_tests: bool,
    /// Exclude configuration files from the tour plan
    #[arg(long)]
    skip_config: bool,
}

#[derive(Debug, Args)]
struct TourArgs {
    /// Print the tour to stdout (pipe-friendly)
    #[arg(long)]
    print: bool,
    /// Tour uncommitted changes
    #[arg(long)]
    uncommitted: bool,
    /// Tour a commit range
    #[arg(long, value_name = "BASE..HEAD")]
    range: Option<String>,
    /// Expand a range tour per commit
    #[arg(long)]
    per_commit: bool,
}

#[derive(Debug, Args)]
#[command(group(
    ArgGroup::new("format")
        .required(true)
        .args(["md", "html"])
))]
struct ExportArgs {
    /// Render to markdown
    #[arg(long)]
    md: bool,
    /// Render to a self-contained HTML document
    #[arg(long)]
    html: bool,
    /// Output path
    #[arg(long, value_name = "PATH")]
    out: Option<PathBuf>,
}

#[derive(Debug, Args)]
struct PromptsArgs {
    /// Print one prompt by name
    #[arg(value_name = "NAME")]
    name: Option<String>,
    /// List prompts without printing their contents
    #[arg(long)]
    list: bool,
    /// Print every prompt
    #[arg(long, alias = "print")]
    all: bool,
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    match cli.command {
        None => {
            let mut command = Cli::command();
            let _ = command.print_help();
            println!();
            ExitCode::SUCCESS
        }
        Some(Command::Serve) => {
            report_prompt_install();
            match laudit_core::rpc::run() {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("laudit serve: {error}");
                    ExitCode::FAILURE
                }
            }
        }
        Some(Command::Prompts(args)) => run_prompts(args),
        Some(Command::Build(args)) => run_build(cli.repo, cli.offline, cli.json, args),
        Some(Command::Tour(args)) => run_tour(cli.repo, cli.offline, cli.color, args),
        Some(Command::Explain { symbol }) => run_explain(cli.repo, cli.offline, cli.color, &symbol),
        Some(Command::Export(args)) => not_implemented("export", &args),
        Some(Command::Ask { question }) => not_implemented("ask", &question),
        Some(Command::Mcp) => not_implemented("mcp", &()),
        Some(Command::Outline) => not_implemented("outline", &()),
        Some(Command::Status { tour }) => run_status(cli.repo, cli.json, tour),
        Some(Command::Clear) => not_implemented("clear", &()),
    }
}

fn not_implemented<T: std::fmt::Debug>(command: &str, _args: &T) -> ExitCode {
    eprintln!("laudit {command}: not implemented yet");
    ExitCode::FAILURE
}

fn run_build(repo: Option<PathBuf>, offline: bool, json: bool, args: BuildArgs) -> ExitCode {
    if args.check {
        return not_implemented("build --check", &args);
    }

    let repo_root = repo.unwrap_or_else(|| PathBuf::from("."));

    if offline {
        let config = match laudit_core::config::Config::load_offline(&repo_root) {
            Ok(config) => config,
            Err(error) => {
                eprintln!("laudit build: {error}");
                return ExitCode::FAILURE;
            }
        };

        return match laudit_core::build::build_offline(&repo_root, &config) {
            Ok(report) => {
                if json {
                    match serde_json::to_string_pretty(&report) {
                        Ok(rendered) => println!("{rendered}"),
                        Err(error) => {
                            eprintln!("laudit build: {error}");
                            return ExitCode::FAILURE;
                        }
                    }
                } else if !args.quiet {
                    print_build_summary(&report);
                }
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("laudit build: {error}");
                ExitCode::FAILURE
            }
        };
    }

    report_prompt_install();
    let config = match laudit_core::config::Config::load(&repo_root) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("laudit build: {error}");
            return ExitCode::FAILURE;
        }
    };
    let client = match laudit_core::llm::LlmClient::from_config(&config) {
        Ok(client) => client,
        Err(error) => {
            eprintln!("laudit build: {error}");
            return ExitCode::FAILURE;
        }
    };
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("laudit build: {error}");
            return ExitCode::FAILURE;
        }
    };

    match runtime.block_on(laudit_core::comprehension::build(
        &repo_root, &config, &client,
    )) {
        Ok(report) => {
            for id in &report.dropped_ids {
                eprintln!("laudit: warning: dropped unknown symbol id {id}");
            }
            for file in &report.dropped_files {
                eprintln!("laudit: warning: dropped unknown file {file}");
            }

            let plan_options = laudit_core::plan::PlanOptions {
                include_tests: args.include_tests,
                include_config: !args.skip_config,
            };
            let plan =
                match laudit_core::plan::plan_from_artifacts(&repo_root, &config, &plan_options) {
                    Ok(plan) => plan,
                    Err(error) => {
                        eprintln!("laudit build: {error}");
                        return ExitCode::FAILURE;
                    }
                };
            for id in &plan.dropped_ids {
                eprintln!("laudit: warning: dropped unknown symbol id {id}");
            }

            if json {
                match serde_json::to_string_pretty(&report) {
                    Ok(rendered) => println!("{rendered}"),
                    Err(error) => {
                        eprintln!("laudit build: {error}");
                        return ExitCode::FAILURE;
                    }
                }
            } else if !args.quiet {
                print_comprehension_summary(&report);
                print_plan_summary(&plan);
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("laudit build: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run_tour(repo: Option<PathBuf>, offline: bool, color: ColorChoice, args: TourArgs) -> ExitCode {
    if args.uncommitted || args.range.is_some() || args.per_commit {
        eprintln!("laudit tour: source adapters are not implemented yet");
        return ExitCode::FAILURE;
    }
    if !args.print {
        eprintln!("laudit tour: interactive tours are not implemented yet; use --print");
        return ExitCode::FAILURE;
    }

    let repo_root = repo.unwrap_or_else(|| PathBuf::from("."));
    let color_enabled = color_enabled(color);
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("laudit tour: {error}");
            return ExitCode::FAILURE;
        }
    };

    if offline {
        let config = match laudit_core::config::Config::load_offline(&repo_root) {
            Ok(config) => config,
            Err(error) => {
                eprintln!("laudit tour: {error}");
                return ExitCode::FAILURE;
            }
        };
        let mut sink = laudit_core::narration::NoopSink;
        return match runtime.block_on(laudit_core::print::prepare(
            &repo_root, &config, true, None, &mut sink,
        )) {
            Ok(prepared) => {
                print!(
                    "{}",
                    laudit_core::print::render(
                        &prepared.tour,
                        laudit_core::print::PrintOptions {
                            color: color_enabled,
                            offline: true,
                        },
                    )
                );
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("laudit tour: {error}");
                ExitCode::FAILURE
            }
        };
    }

    report_prompt_install();
    let config = match laudit_core::config::Config::load(&repo_root) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("laudit tour: {error}");
            return ExitCode::FAILURE;
        }
    };
    let client = match laudit_core::llm::LlmClient::from_config(&config) {
        Ok(client) => client,
        Err(error) => {
            eprintln!("laudit tour: {error}");
            return ExitCode::FAILURE;
        }
    };

    let mut sink = laudit_core::narration::NoopSink;
    match runtime.block_on(laudit_core::print::prepare(
        &repo_root,
        &config,
        false,
        Some(&client),
        &mut sink,
    )) {
        Ok(prepared) => {
            for warning in &prepared.warnings {
                eprintln!("laudit: warning: {warning}");
            }
            print!(
                "{}",
                laudit_core::print::render(
                    &prepared.tour,
                    laudit_core::print::PrintOptions {
                        color: color_enabled,
                        offline: false,
                    },
                )
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("laudit tour: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run_explain(repo: Option<PathBuf>, offline: bool, color: ColorChoice, symbol: &str) -> ExitCode {
    let repo_root = repo.unwrap_or_else(|| PathBuf::from("."));
    let color_enabled = color_enabled(color);

    if offline {
        let config = match laudit_core::config::Config::load_offline(&repo_root) {
            Ok(config) => config,
            Err(error) => {
                eprintln!("laudit explain: {error}");
                return ExitCode::FAILURE;
            }
        };
        let skeleton = match laudit_core::explain::load_skeleton(&repo_root, &config) {
            Ok(skeleton) => skeleton,
            Err(error) => {
                eprintln!("laudit explain: {error}");
                return ExitCode::FAILURE;
            }
        };
        let resolved = match laudit_core::explain::resolve(&skeleton, symbol) {
            Ok(resolved) => resolved,
            Err(error) => {
                eprintln!("laudit explain: {error}");
                return ExitCode::FAILURE;
            }
        };
        let Some(node) = skeleton.symbols.iter().find(|node| node.id == resolved.id) else {
            eprintln!(
                "laudit explain: symbol {} is no longer in the index",
                resolved.id
            );
            return ExitCode::FAILURE;
        };
        let stop = match laudit_core::plan::stop_for_symbol(&repo_root, &skeleton, node) {
            Ok(stop) => stop,
            Err(error) => {
                eprintln!("laudit explain: {error}");
                return ExitCode::FAILURE;
            }
        };
        print_explain_offline(&resolved, &stop, color_enabled);
        return ExitCode::SUCCESS;
    }

    report_prompt_install();
    let config = match laudit_core::config::Config::load(&repo_root) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("laudit explain: {error}");
            return ExitCode::FAILURE;
        }
    };
    let client = match laudit_core::llm::LlmClient::from_config(&config) {
        Ok(client) => client,
        Err(error) => {
            eprintln!("laudit explain: {error}");
            return ExitCode::FAILURE;
        }
    };
    let skeleton = match laudit_core::explain::load_skeleton(&repo_root, &config) {
        Ok(skeleton) => skeleton,
        Err(error) => {
            eprintln!("laudit explain: {error}");
            return ExitCode::FAILURE;
        }
    };
    let model = match laudit_core::explain::load_model(&repo_root) {
        Ok(model) => model,
        Err(error) => {
            eprintln!("laudit explain: {error}");
            return ExitCode::FAILURE;
        }
    };
    let resolved = match laudit_core::explain::resolve(&skeleton, symbol) {
        Ok(resolved) => resolved,
        Err(error) => {
            eprintln!("laudit explain: {error}");
            return ExitCode::FAILURE;
        }
    };
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("laudit explain: {error}");
            return ExitCode::FAILURE;
        }
    };

    let mut sink = laudit_core::narration::NoopSink;
    match runtime.block_on(laudit_core::explain::explain(
        &repo_root, &config, &skeleton, &model, &resolved, &client, &mut sink,
    )) {
        Ok(report) => {
            for warning in &report.warnings {
                eprintln!("laudit: warning: {warning}");
            }
            print_explain(&report, color_enabled);
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("laudit explain: {error}");
            ExitCode::FAILURE
        }
    }
}

fn color_enabled(color: ColorChoice) -> bool {
    match color {
        ColorChoice::Always => true,
        ColorChoice::Never => false,
        ColorChoice::Auto => std::io::stdout().is_terminal(),
    }
}

fn print_explain(report: &laudit_core::explain::ExplainReport, color: bool) {
    let stop = &report.stop;
    let title = if stop.title.is_empty() {
        &stop.symbol_id
    } else {
        &stop.title
    };
    let location = format!("{}:{}-{}", stop.file, stop.start_line, stop.end_line);
    if color {
        println!("\x1b[1m{title}\x1b[0m");
        println!("\x1b[2m{location}\x1b[0m");
    } else {
        println!("{title}");
        println!("{location}");
    }
    if !stop.why_here.is_empty() {
        println!();
        println!("Why: {}", stop.why_here);
    }
    if !stop.explanation.is_empty() {
        println!();
        print!("{}", stop.explanation);
        if !stop.explanation.ends_with('\n') {
            println!();
        }
    }
}

fn print_explain_offline(
    symbol: &laudit_core::explain::ResolvedSymbol,
    stop: &laudit_core::model::Stop,
    color: bool,
) {
    let title = if stop.title.is_empty() {
        &symbol.name
    } else {
        &stop.title
    };
    let location = format!("{}:{}-{}", stop.file, stop.start_line, stop.end_line);
    if color {
        println!("\x1b[1m{title}\x1b[0m");
        println!("\x1b[2m{location}\x1b[0m");
    } else {
        println!("{title}");
        println!("{location}");
    }
}

fn run_status(repo: Option<PathBuf>, json: bool, tour: Option<String>) -> ExitCode {
    let repo_root = repo.unwrap_or_else(|| PathBuf::from("."));
    match laudit_core::status::status(&repo_root, tour.as_deref()) {
        Ok(report) => {
            if json {
                match serde_json::to_string_pretty(&report) {
                    Ok(rendered) => println!("{rendered}"),
                    Err(error) => {
                        eprintln!("laudit status: {error}");
                        return ExitCode::FAILURE;
                    }
                }
            } else {
                print_status_summary(&report);
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("laudit status: {error}");
            ExitCode::FAILURE
        }
    }
}

fn print_status_summary(report: &laudit_core::status::StatusReport) {
    println!("laudit status");
    println!(
        "  model:       {}",
        if report.model_present {
            "present"
        } else {
            "missing"
        }
    );
    match &report.tour_path {
        Some(path) => println!("  tour:        {}", path.display()),
        None => println!("  tour:        none"),
    }
    if let Some(position) = report.position {
        println!("  position:    {}:{}", position.chapter, position.stop);
    }
    println!("  chapters:    {}", report.chapters);
    println!("  stops:       {}", report.stops);
    println!("  explained:   {}", report.explained);
    println!("  pending:     {}", report.pending);
    println!("  streaming:   {}", report.streaming);
    println!("  skipped:     {}", report.skipped);
    println!("  failed:      {}", report.failed);
    println!("  stale:       {}", report.stale);
}

fn print_plan_summary(report: &laudit_core::plan::PlanReport) {
    let stops: usize = report
        .tour
        .chapters
        .iter()
        .map(|chapter| chapter.stops.len())
        .sum();
    println!("  tour:         {}", report.tour_path.display());
    println!("  tour chapters: {}", report.tour.chapters.len());
    println!("  tour stops:   {stops}");
}

fn print_comprehension_summary(report: &laudit_core::comprehension::ComprehensionReport) {
    println!("laudit build");
    println!("  model:        {}", report.model_path.display());
    println!("  subsystems:   {}", report.model.subsystems.len());
    println!("  chapters:     {}", report.model.chapters.len());
    println!("  flows:        {}", report.model.flows.len());
    println!("  glossary:     {}", report.model.glossary.len());
    if report.cached {
        println!("  cache:        hit");
    } else if report.hierarchical {
        println!("  cache:        miss (hierarchical)");
    } else {
        println!("  cache:        miss");
    }
}

fn print_build_summary(report: &laudit_core::build::BuildReport) {
    println!("laudit build (offline)");
    println!("  files:       {}", report.files);
    println!("  symbols:     {}", report.symbols);
    println!("  communities: {}", report.communities);
    println!("  entrypoints: {}", report.entrypoints);
    println!(
        "  parsed:      {} (reused {})",
        report.parsed, report.reused
    );
    println!("  index:       {}", report.index_path.display());
    println!("  skeleton:    {}", report.skeleton_path.display());
    println!("  model:       {}", report.model_path.display());
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

fn run_prompts(args: PromptsArgs) -> ExitCode {
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

    if let Some(name) = args.name {
        return print_prompt(&name);
    }

    if args.all {
        return print_all_prompts();
    }

    if let Err(error) = list_prompts() {
        eprintln!("laudit prompts: {error}");
        return ExitCode::FAILURE;
    }

    if args.list {
        return ExitCode::SUCCESS;
    }

    println!();
    print_all_prompts()
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
