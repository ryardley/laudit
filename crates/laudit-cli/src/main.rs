use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
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
    #[arg(long, conflicts_with = "md")]
    print: bool,
    /// Render the tour as markdown
    #[arg(long, conflicts_with = "print")]
    md: bool,
    /// Write markdown to a file (requires --md)
    #[arg(long, requires = "md", value_name = "PATH")]
    out: Option<PathBuf>,
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
    /// Output file, or directory (writes index.html)
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
        Some(Command::Export(args)) => run_export(cli.repo, args),
        Some(Command::Ask { question }) => run_ask(cli.repo, cli.offline, cli.json, question),
        Some(Command::Mcp) => {
            report_prompt_install();
            match laudit_mcp::run() {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("laudit mcp: {error}");
                    ExitCode::FAILURE
                }
            }
        }
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
    let repo_root = repo.unwrap_or_else(|| PathBuf::from("."));

    if args.check {
        return run_build_check(&repo_root, json, args.quiet);
    }

    report_config_install();

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

fn run_build_check(repo_root: &Path, json: bool, quiet: bool) -> ExitCode {
    let config = match laudit_core::config::Config::load_offline(repo_root) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("laudit build: {error}");
            return ExitCode::FAILURE;
        }
    };

    match laudit_core::build::check_offline(repo_root, &config) {
        Ok(report) => {
            if json {
                match serde_json::to_string_pretty(&report) {
                    Ok(rendered) => println!("{rendered}"),
                    Err(error) => {
                        eprintln!("laudit build: {error}");
                        return ExitCode::FAILURE;
                    }
                }
            } else if !quiet {
                if report.fresh {
                    println!("laudit build (offline): artifacts are fresh");
                } else {
                    println!("laudit build (offline): artifacts are stale");
                    for artifact in &report.stale {
                        println!(
                            "  {}: {}",
                            artifact.path.display(),
                            stale_reason(artifact.reason)
                        );
                    }
                }
            }

            if report.fresh {
                ExitCode::SUCCESS
            } else {
                if quiet {
                    for artifact in &report.stale {
                        eprintln!(
                            "laudit build: stale artifact {} ({})",
                            artifact.path.display(),
                            stale_reason(artifact.reason)
                        );
                    }
                }
                ExitCode::FAILURE
            }
        }
        Err(error) => {
            eprintln!("laudit build: {error}");
            ExitCode::FAILURE
        }
    }
}

fn stale_reason(reason: laudit_core::build::StaleReason) -> &'static str {
    match reason {
        laudit_core::build::StaleReason::Missing => "missing",
        laudit_core::build::StaleReason::Changed => "changed",
    }
}

fn run_tour(repo: Option<PathBuf>, offline: bool, color: ColorChoice, args: TourArgs) -> ExitCode {
    let repo_root = repo.unwrap_or_else(|| PathBuf::from("."));

    let source = if args.uncommitted {
        if args.range.is_some() || args.per_commit {
            eprintln!("laudit tour: --uncommitted cannot be combined with --range or --per-commit");
            return ExitCode::FAILURE;
        }
        Some(laudit_core::source::SourceSpec::Uncommitted)
    } else if let Some(range) = &args.range {
        match laudit_core::source::SourceSpec::parse_range(range, args.per_commit) {
            Ok(spec) => Some(spec),
            Err(error) => {
                eprintln!("laudit tour: {error}");
                return ExitCode::FAILURE;
            }
        }
    } else if args.per_commit {
        eprintln!("laudit tour: --per-commit requires --range <base..head>");
        return ExitCode::FAILURE;
    } else {
        None
    };

    if let Some(spec) = source {
        return run_source_tour(repo_root, offline, color, &args, spec);
    }

    if args.md {
        return run_codebase_markdown(repo_root, offline, &args);
    }

    if !args.print {
        return match laudit_tui::run(repo_root, offline) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("laudit tour: {error}");
                ExitCode::FAILURE
            }
        };
    }

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

fn run_codebase_markdown(repo_root: PathBuf, offline: bool, args: &TourArgs) -> ExitCode {
    let config = if offline {
        match laudit_core::config::Config::load_offline(&repo_root) {
            Ok(config) => config,
            Err(error) => {
                eprintln!("laudit tour: {error}");
                return ExitCode::FAILURE;
            }
        }
    } else {
        match laudit_core::config::Config::load(&repo_root) {
            Ok(config) => config,
            Err(error) => {
                eprintln!("laudit tour: {error}");
                return ExitCode::FAILURE;
            }
        }
    };

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

    let prepared = if offline {
        let mut sink = laudit_core::narration::NoopSink;
        runtime.block_on(laudit_core::print::prepare(
            &repo_root, &config, true, None, &mut sink,
        ))
    } else {
        report_prompt_install();
        let client = match laudit_core::llm::LlmClient::from_config(&config) {
            Ok(client) => client,
            Err(error) => {
                eprintln!("laudit tour: {error}");
                return ExitCode::FAILURE;
            }
        };
        let mut sink = laudit_core::narration::NoopSink;
        runtime.block_on(laudit_core::print::prepare(
            &repo_root,
            &config,
            false,
            Some(&client),
            &mut sink,
        ))
    };

    match prepared {
        Ok(prepared) => {
            for warning in &prepared.warnings {
                eprintln!("laudit: warning: {warning}");
            }
            let report = match laudit_core::export::export_markdown(
                &repo_root,
                Some(&prepared.tour.tour_id),
                None,
            ) {
                Ok(report) => report,
                Err(error) => {
                    eprintln!("laudit tour: {error}");
                    return ExitCode::FAILURE;
                }
            };
            let markdown = match redact_markdown(&report.markdown, &config) {
                Ok(markdown) => markdown,
                Err(error) => {
                    eprintln!("laudit tour: {error}");
                    return ExitCode::FAILURE;
                }
            };
            write_markdown(&markdown, args.out.as_deref())
        }
        Err(error) => {
            eprintln!("laudit tour: {error}");
            ExitCode::FAILURE
        }
    }
}

fn redact_markdown(
    markdown: &str,
    config: &laudit_core::config::Config,
) -> Result<String, laudit_core::redact::RedactError> {
    let (redacted, _) = laudit_core::redact::redact_with_config(markdown, &config.redaction)?;
    Ok(redacted)
}

fn write_markdown(markdown: &str, out: Option<&Path>) -> ExitCode {
    match out {
        Some(path) if path != Path::new("-") => {
            if let Some(parent) = path.parent() {
                if !parent.as_os_str().is_empty() {
                    if let Err(error) = std::fs::create_dir_all(parent) {
                        eprintln!(
                            "laudit tour: failed to create {}: {error}",
                            parent.display()
                        );
                        return ExitCode::FAILURE;
                    }
                }
            }
            match std::fs::write(path, markdown) {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("laudit tour: failed to write {}: {error}", path.display());
                    ExitCode::FAILURE
                }
            }
        }
        _ => {
            print!("{markdown}");
            ExitCode::SUCCESS
        }
    }
}

fn run_source_tour(
    repo_root: PathBuf,
    offline: bool,
    color: ColorChoice,
    args: &TourArgs,
    spec: laudit_core::source::SourceSpec,
) -> ExitCode {
    let config = if offline {
        match laudit_core::config::Config::load_offline(&repo_root) {
            Ok(config) => config,
            Err(error) => {
                eprintln!("laudit tour: {error}");
                return ExitCode::FAILURE;
            }
        }
    } else {
        match laudit_core::config::Config::load(&repo_root) {
            Ok(config) => config,
            Err(error) => {
                eprintln!("laudit tour: {error}");
                return ExitCode::FAILURE;
            }
        }
    };

    let recon = match laudit_core::build::recon(&repo_root, &config) {
        Ok(recon) => recon,
        Err(error) => {
            eprintln!("laudit tour: {error}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(error) = laudit_core::build::write_skeleton(&repo_root, &recon.skeleton) {
        eprintln!("laudit tour: {error}");
        return ExitCode::FAILURE;
    }

    let model = match laudit_core::plan::load_model(&repo_root) {
        Ok(model) => model,
        Err(laudit_core::plan::PlanError::Io { source, .. })
            if source.kind() == std::io::ErrorKind::NotFound =>
        {
            laudit_core::build::offline_model(&recon.skeleton)
        }
        Err(error) => {
            eprintln!("laudit tour: {error}");
            return ExitCode::FAILURE;
        }
    };

    let report =
        match laudit_core::source::plan(&repo_root, &config, &recon.skeleton, &model, &spec) {
            Ok(report) => report,
            Err(error) => {
                eprintln!("laudit tour: {error}");
                return ExitCode::FAILURE;
            }
        };
    for warning in &report.warnings {
        eprintln!("laudit: warning: {warning}");
    }

    if !args.md && !args.print {
        return match laudit_tui::run(repo_root, offline) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("laudit tour: {error}");
                ExitCode::FAILURE
            }
        };
    }

    let tour = if offline {
        report.tour
    } else {
        report_prompt_install();
        let client = match laudit_core::llm::LlmClient::from_config(&config) {
            Ok(client) => client,
            Err(error) => {
                eprintln!("laudit tour: {error}");
                return ExitCode::FAILURE;
            }
        };
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

        let mut sink = laudit_core::narration::NoopSink;
        match runtime.block_on(laudit_core::print::prepare_tour(
            &repo_root,
            &config,
            false,
            Some(&client),
            &mut sink,
            report.tour,
        )) {
            Ok(prepared) => {
                for warning in &prepared.warnings {
                    eprintln!("laudit: warning: {warning}");
                }
                prepared.tour
            }
            Err(error) => {
                eprintln!("laudit tour: {error}");
                return ExitCode::FAILURE;
            }
        }
    };

    if args.md {
        let markdown = match laudit_core::export::render_markdown(
            &repo_root,
            &tour,
            &model,
            Some(&recon.skeleton),
            &laudit_core::export::MarkdownOptions::default(),
        ) {
            Ok(markdown) => markdown,
            Err(error) => {
                eprintln!("laudit tour: {error}");
                return ExitCode::FAILURE;
            }
        };
        let markdown = match redact_markdown(&markdown, &config) {
            Ok(markdown) => markdown,
            Err(error) => {
                eprintln!("laudit tour: {error}");
                return ExitCode::FAILURE;
            }
        };
        return write_markdown(&markdown, args.out.as_deref());
    }

    let color_enabled = color_enabled(color);
    print!(
        "{}",
        laudit_core::print::render(
            &tour,
            laudit_core::print::PrintOptions {
                color: color_enabled,
                offline,
            },
        )
    );
    ExitCode::SUCCESS
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

fn run_ask(repo: Option<PathBuf>, offline: bool, json: bool, question: Option<String>) -> ExitCode {
    let repo_root = repo.unwrap_or_else(|| PathBuf::from("."));

    if offline {
        return run_ask_offline(&repo_root, question);
    }

    report_prompt_install();
    let config = match laudit_core::config::Config::load(&repo_root) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("laudit ask: {error}");
            return ExitCode::FAILURE;
        }
    };
    let client = match laudit_core::llm::LlmClient::from_config(&config) {
        Ok(client) => client,
        Err(error) => {
            eprintln!("laudit ask: {error}");
            return ExitCode::FAILURE;
        }
    };
    let skeleton = match laudit_core::explain::load_skeleton(&repo_root, &config) {
        Ok(skeleton) => skeleton,
        Err(error) => {
            eprintln!("laudit ask: {error}");
            return ExitCode::FAILURE;
        }
    };
    let model = match laudit_core::explain::load_model(&repo_root) {
        Ok(model) => model,
        Err(error) => {
            eprintln!("laudit ask: {error}");
            return ExitCode::FAILURE;
        }
    };
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("laudit ask: {error}");
            return ExitCode::FAILURE;
        }
    };

    let session = AskSession {
        runtime: &runtime,
        repo_root: &repo_root,
        config: &config,
        skeleton: &skeleton,
        model: &model,
        client: &client,
        json,
    };

    match question {
        Some(question) => match ask_question(&session, &question) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("laudit ask: {error}");
                ExitCode::FAILURE
            }
        },
        None => run_ask_repl(&session),
    }
}

struct AskSession<'a> {
    runtime: &'a tokio::runtime::Runtime,
    repo_root: &'a Path,
    config: &'a laudit_core::config::Config,
    skeleton: &'a laudit_core::model::Skeleton,
    model: &'a laudit_core::model::CodebaseModel,
    client: &'a laudit_core::llm::LlmClient,
    json: bool,
}

fn ask_question(
    session: &AskSession<'_>,
    question: &str,
) -> Result<(), laudit_core::ask::AskError> {
    let report = if session.json {
        let mut sink = laudit_core::ask::NoopAskSink;
        session.runtime.block_on(laudit_core::ask::ask(
            session.repo_root,
            session.config,
            session.skeleton,
            session.model,
            question,
            session.client,
            &mut sink,
        ))?
    } else {
        let mut sink = StdoutAskSink;
        session.runtime.block_on(laudit_core::ask::ask(
            session.repo_root,
            session.config,
            session.skeleton,
            session.model,
            question,
            session.client,
            &mut sink,
        ))?
    };

    for warning in &report.warnings {
        eprintln!("laudit: warning: {warning}");
    }

    if session.json {
        match serde_json::to_string_pretty(&report) {
            Ok(rendered) => println!("{rendered}"),
            Err(error) => eprintln!("laudit ask: {error}"),
        }
        return Ok(());
    }

    if !report.answer.ends_with('\n') {
        println!();
    }
    if report.citations.is_empty() && !answer_is_unknown(&report.answer) {
        println!();
        println!("Sources:");
        for symbol in &report.retrieved {
            println!("  {}:{}", symbol.file, symbol.line);
        }
    }
    Ok(())
}

fn run_ask_repl(session: &AskSession<'_>) -> ExitCode {
    let stdin = std::io::stdin();
    let interactive = stdin.is_terminal();
    let mut line = String::new();
    loop {
        if interactive {
            print!("laudit ask> ");
            let _ = std::io::stdout().flush();
        }
        line.clear();
        match stdin.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {
                let question = line.trim();
                if question.is_empty() {
                    continue;
                }
                if matches!(question, "exit" | "quit" | ":q" | "q") {
                    break;
                }
                if let Err(error) = ask_question(session, question) {
                    eprintln!("laudit ask: {error}");
                }
            }
            Err(error) => {
                eprintln!("laudit ask: {error}");
                return ExitCode::FAILURE;
            }
        }
    }
    ExitCode::SUCCESS
}

fn run_ask_offline(repo_root: &Path, question: Option<String>) -> ExitCode {
    let config = match laudit_core::config::Config::load_offline(repo_root) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("laudit ask: {error}");
            return ExitCode::FAILURE;
        }
    };
    let skeleton = match laudit_core::explain::load_skeleton(repo_root, &config) {
        Ok(skeleton) => skeleton,
        Err(error) => {
            eprintln!("laudit ask: {error}");
            return ExitCode::FAILURE;
        }
    };
    let model = match laudit_core::explain::load_model(repo_root) {
        Ok(model) => model,
        Err(error) => {
            eprintln!("laudit ask: {error}");
            return ExitCode::FAILURE;
        }
    };
    let index = laudit_core::recon::load_index(repo_root).ok().flatten();
    let options = laudit_core::ask::AskOptions::default();
    let ask = |question: &str| {
        let retrieved = laudit_core::ask::retrieve(
            repo_root,
            &skeleton,
            &model,
            index.as_ref(),
            question,
            &options,
        );
        print_retrieved(&retrieved);
    };

    match question {
        Some(question) => {
            ask(&question);
            ExitCode::SUCCESS
        }
        None => {
            let stdin = std::io::stdin();
            let interactive = stdin.is_terminal();
            let mut line = String::new();
            loop {
                if interactive {
                    print!("laudit ask> ");
                    let _ = std::io::stdout().flush();
                }
                line.clear();
                match stdin.read_line(&mut line) {
                    Ok(0) => break,
                    Ok(_) => {
                        let question = line.trim();
                        if question.is_empty() {
                            continue;
                        }
                        if matches!(question, "exit" | "quit" | ":q" | "q") {
                            break;
                        }
                        ask(question);
                    }
                    Err(error) => {
                        eprintln!("laudit ask: {error}");
                        return ExitCode::FAILURE;
                    }
                }
            }
            ExitCode::SUCCESS
        }
    }
}

fn print_retrieved(retrieved: &[laudit_core::ask::RetrievedSymbol]) {
    if retrieved.is_empty() {
        println!("I don't know — no relevant symbols were found.");
        return;
    }
    println!("Relevant symbols:");
    for symbol in retrieved {
        println!(
            "  {}:{}  {} ({:.2})",
            symbol.file, symbol.line, symbol.name, symbol.score
        );
    }
}

fn answer_is_unknown(answer: &str) -> bool {
    let lower = answer.to_lowercase();
    [
        "don't know",
        "do not know",
        "not enough",
        "insufficient",
        "cannot answer",
        "can't answer",
        "no evidence",
    ]
    .iter()
    .any(|phrase| lower.contains(phrase))
}

struct StdoutAskSink;

impl laudit_core::ask::AskSink for StdoutAskSink {
    fn on_chunk(&mut self, delta: &str) {
        print!("{delta}");
        let _ = std::io::stdout().flush();
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

fn run_export(repo: Option<PathBuf>, args: ExportArgs) -> ExitCode {
    let repo_root = repo.unwrap_or_else(|| PathBuf::from("."));
    let out = args
        .out
        .as_deref()
        .filter(|path| *path != std::path::Path::new("-"));

    if args.html {
        return match laudit_core::export::export_html(&repo_root, None, out) {
            Ok(report) => {
                if out.is_none() {
                    print!("{}", report.html);
                }
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("laudit export: {error}");
                ExitCode::FAILURE
            }
        };
    }

    match laudit_core::export::export_markdown(&repo_root, None, out) {
        Ok(report) => {
            if out.is_none() {
                print!("{}", report.markdown);
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("laudit export: {error}");
            ExitCode::FAILURE
        }
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

fn report_config_install() {
    match laudit_core::config::ensure_installed() {
        Ok(report) if report.installed => {
            eprintln!("laudit: installed config at {}", report.path.display());
        }
        Ok(_) => {}
        Err(error) => eprintln!("laudit: warning: could not install config: {error}"),
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
