use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;
use std::process::{exit, Stdio};
use tracing::{debug, error, info, trace, warn};
use tracing_subscriber::{filter::LevelFilter, EnvFilter, FmtSubscriber};

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Cli {
    #[command(flatten)]
    globals: Globals,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Args)]
struct Globals {
    /// Sets a custom config file
    #[arg(short, long, value_name = "FILE", global = true)]
    config: Option<PathBuf>,

    /// Turn debugging information on
    #[arg(short, long, action = clap::ArgAction::Count, global = true)]
    debug: u8,
}

#[derive(Subcommand)]
enum Commands {
    /// Tokenize a file or stdin.
    ///
    /// Tokens are written to stdout, one token per line.
    Tokenize {
        /// The file or files to tokenize.
        ///
        /// If no files are specified, this command will read from standard input.
        files: Vec<PathBuf>,

        /// Don't actually output the tokens, just perform the tokenization and count how many
        /// tokens there are.
        #[arg(long)]
        count: bool,
    },
}

impl Commands {
    async fn execute(self, globals: &Globals) -> anyhow::Result<()> {
        use Commands::*;
        match self {
            Tokenize { files, count } => {
                let _tokens = if !files.is_empty() {
                    info!("Printing testing lists...");
                    info!("  {files:?}");
                    aiknife::tokenize_files(aiknife::Tokenizer::Cl100kBase, files).await?;
                } else {
                    info!("Not printing testing lists...");
                    aiknife::tokenize_stream(aiknife::Tokenizer::Cl100kBase, std::io::stdin())
                        .await?;
                };

                if count {
                    todo!()
                } else {
                    todo!()
                }
            }
        }
    }
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    // Initialize tracing with JSON formatting and full detail
    let subscriber = FmtSubscriber::builder()
        .with_env_filter(
            EnvFilter::builder()
                .with_default_directive(LevelFilter::INFO.into())
                .from_env_lossy(),
        )
        .json()
        .finish();

    tracing::subscriber::set_global_default(subscriber).expect("Failed to set subscriber");

    // You can check the value provided by positional arguments, or option arguments
    if let Some(config_path) = cli.globals.config.as_deref() {
        info!("Value for config: {}", config_path.display());
    }

    // You can see how many times a particular flag or argument occurred
    // Note, only flags can have multiple occurrences
    match cli.globals.debug {
        0 => info!("Debug mode is off"),
        1 => info!("Debug mode is kind of on"),
        2 => info!("Debug mode is on"),
        _ => info!("Don't be crazy"),
    }

    match cli.command {
        None => {
            // TODO: once there's an obvious reasonable default command, use that
            panic!("TODO: make a default command");
        }
        Some(command) => {
            if let Err(e) = command.execute(&cli.globals).await {
                error!("{:#}", e);
                exit(1);
            } else {
                debug!("command executed successfully");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verify that there aren't any invalid attributes in the CLI specification that can only be
    /// detected at runtime
    #[test]
    fn verify_cli() {
        use clap::CommandFactory;
        Cli::command().debug_assert()
    }
}
