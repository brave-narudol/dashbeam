use anyhow::{ensure, Result};
use clap::{Parser, Subcommand};
use dashbeam_inbox::{
    config::{self, InboxCredential, ReceiverConfig},
    send_files, start_receiver,
};
use std::{path::PathBuf, time::Duration};

#[derive(Parser)]
#[command(version, about = "A token-protected, upload-only file inbox")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Generate receiver configuration and a separate credential to give senders.
    Init {
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        credential: PathBuf,
    },
    /// Receive uploads until Ctrl+C. No pairing or file-serving interface is enabled.
    Listen {
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        output: PathBuf,
        #[arg(long, default_value_t = 3600)]
        timeout_seconds: u64,
    },
    /// Upload files or directories using a credential issued by the receiver.
    Send {
        #[arg(long)]
        credential: PathBuf,
        #[arg(long, default_value_t = 3600)]
        timeout_seconds: u64,
        #[arg(required = true)]
        paths: Vec<PathBuf>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Init { config, credential } => {
            config::initialize(&config, &credential)?;
            println!("Receiver config: {}", config.display());
            println!("Sender credential: {}", credential.display());
            println!("Keep the receiver config private. Share only the sender credential.");
        }
        Command::Listen {
            config,
            output,
            timeout_seconds,
        } => {
            ensure!(timeout_seconds > 0, "timeout must be positive");
            let receiver: ReceiverConfig = config::read_config(&config)?;
            let router = start_receiver(
                receiver,
                output,
                false,
                Duration::from_secs(timeout_seconds),
            )
            .await?;
            println!(
                "Inbox {} is listening. Press Ctrl+C to stop.",
                router.endpoint().id()
            );
            let stop = tokio::signal::ctrl_c().await;
            router.shutdown().await?;
            stop?;
        }
        Command::Send {
            credential,
            paths,
            timeout_seconds,
        } => {
            ensure!(timeout_seconds > 0, "timeout must be positive");
            let credential: InboxCredential = config::read_config(&credential)?;
            credential.validate()?;
            let target =
                iroh::EndpointAddr::from(credential.endpoint_id.parse::<iroh::EndpointId>()?);
            let id = send_files(
                &credential,
                target,
                paths,
                false,
                Duration::from_secs(timeout_seconds),
            )
            .await?;
            println!("Upload complete: {id}");
        }
    }
    Ok(())
}
