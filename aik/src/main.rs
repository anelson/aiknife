use aiknife::audio;
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
        }

        Ok(())
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
        .with_writer(std::io::stderr)
        .finish();

    tracing::subscriber::set_global_default(subscriber).expect("Failed to set subscriber");

    // You can check the value provided by positional arguments, or option arguments
    if let Some(config_path) = cli.globals.config.as_deref() {
        debug!("Value for config: {}", config_path.display());
    }

    // You can see how many times a particular flag or argument occurred
    // Note, only flags can have multiple occurrences
    match cli.globals.debug {
        0 => debug!("Debug mode is off"),
        1 => debug!("Debug mode is kind of on"),
        2 => debug!("Debug mode is on"),
        _ => debug!("Don't be crazy"),
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
