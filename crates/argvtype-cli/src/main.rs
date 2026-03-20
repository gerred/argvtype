mod commands;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "argvtype", about = "A static type-and-effect checker for Bash")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Check Bash files for type errors
    Check {
        /// Files to check
        paths: Vec<String>,
        /// Output format
        #[arg(long, default_value = "text")]
        format: String,
        /// Dump HIR to stdout
        #[arg(long)]
        dump_hir: bool,
        /// Check a command string directly
        #[arg(long, short = 'c')]
        command: Option<String>,
        /// Read source from stdin
        #[arg(long)]
        stdin: bool,
        /// Output structured JSON for AI agent consumers
        #[arg(long)]
        agent: bool,
    },
    /// Start the language server
    Lsp,
    /// Explain a diagnostic code
    Explain {
        /// Diagnostic code (e.g., BT201)
        code: String,
    },
}

fn main() {
    let cli = Cli::parse();

    let exit_code = match cli.command {
        Commands::Check {
            paths,
            format,
            dump_hir,
            command,
            stdin,
            agent,
        } => commands::check::run(&paths, &format, dump_hir, command.as_deref(), stdin, agent),
        Commands::Lsp => match argvtype_lsp::run_server() {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("{}", e);
                1
            }
        },
        Commands::Explain { code } => {
            eprintln!("explain is not yet implemented for code: {}", code);
            1
        }
    };

    std::process::exit(exit_code);
}
