use aiknife::{audio, mcp};
use clap::{Args, Parser, Subcommand};
use std::io::Write;
use std::path::PathBuf;
use std::process::exit;
use tracing::*;
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
    /// Use Whisper to transcribe audio to text
    Whisper {
        #[command(subcommand)]
        command: WhisperCommands,
    },

    /// Run an MCP server, listening on stdin and responding on stdout
    /// Log events are written to stderr
    Mcp {
        /// Listen on the specified Unix domain socket
        ///
        /// If not specified, the default behavior is to listen on stdin and respond on stdout.
        socket: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum WhisperCommands {
    /// List all available audio devices
    ListDevices,

    /// Transcribe audio using the Whisper model running locally
    Transcribe {
        /// Use a specific audio device specified by name.
        ///
        /// If no device or file is specified, the default behavior is to use the default input device.
        #[arg(long, group = "audio_source")]
        device: Option<String>,

        /// Read the audio from a file on the filesystem.
        /// Most audio formats are supported.
        ///
        /// If no file or device is specified, the default behavior is to use the default input device.
        #[arg(long, group = "audio_source")]
        file: Option<PathBuf>,
        // TODO: Decide what if any more fields from audio::AudioInputConfig should be exposed here
    },
}

impl Commands {
    async fn execute(self, globals: &Globals) -> anyhow::Result<()> {
        use Commands::*;
        match self {
            Whisper { command } => match command {
                // TODO: All whisper operations are blocking.  Should they be run in
                // `tokio::spawn_blocking`?
                WhisperCommands::ListDevices => {
                    println!("Listing devices");
                    let (input, output) = audio::list_device_names()?;

                    println!("Input devices:");
                    for device in input {
                        println!("  {}", device);
                    }

                    println!("Output devices:");
                    for device in output {
                        println!("  {}", device);
                    }
                }
                WhisperCommands::Transcribe { device, file } => {
                    let source = if let Some(device) = device {
                        audio::AudioSource::Device(device)
                    } else if let Some(file) = file {
                        audio::AudioSource::File(file)
                    } else {
                        debug!("No audio source specified, attempting to use system default input device");
                        audio::AudioSource::Default
                    };

                    let config = audio::AudioInputConfig {
                        source,
                        ..Default::default()
                    };

                    let mut input = audio::open_audio_input(config.source.clone())?;

                    let (mut guard, stream) = input.start_stream(config)?;

                    println!("Listening to {}", stream.device_name());
                    ctrlc::set_handler(move || {
                        println!("Ctrl-C detected; Stopping stream...");
                        guard.stop_stream();
                    })?;

                    let mut total_samples = 0;
                    for result in stream {
                        let chunk = result?;
                        total_samples += chunk.samples.len();
                        print!("\rRead samples: {}", total_samples);
                        std::io::stdout().flush()?;
                    }
                    println!("\rRead {total_samples} samples in total");
                }
            },
            Mcp { socket } => {
                let transport: Box<dyn mcp::McpTransport> = match socket {
                    Some(path) => {
                        let transport = mcp::UnixSocketTransport::bind(&path).await?;
                        info!("Listening on socket {}", path.display());
                        Box::new(transport)
                    }
                    None => {
                        let stdin = tokio::io::stdin();
                        let stdout = tokio::io::stdout();
                        let transport = mcp::StdioTransport::stdio(stdin, stdout);
                        info!("Listening on stdin");
                        Box::new(transport)
                    }
                };

                aiknife::mcp::serve(transport).await?;
            }
        }

        Ok(())
    }
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    // You can see how many times a particular flag or argument occurred
    // Note, only flags can have multiple occurrences
    let default_log_directive = match cli.globals.debug {
        0 => LevelFilter::WARN,
        1 => LevelFilter::INFO,
        2 => LevelFilter::DEBUG,
        _ => LevelFilter::TRACE,
    };

    // Initialize tracing with JSON formatting and full detail
    let subscriber = FmtSubscriber::builder()
        .with_env_filter(
            EnvFilter::builder()
                .with_default_directive(default_log_directive.into())
                .from_env_lossy(),
        )
        .json()
        .with_writer(std::io::stderr)
        .finish();

    tracing::subscriber::set_global_default(subscriber).expect("Failed to set subscriber");

    // You can check the value provided by positional arguments, or option arguments
    if let Some(config_path) = cli.globals.config.as_deref() {
        debug!("Value for config: {}", config_path.display());
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
