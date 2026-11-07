use anyhow::{ensure, Result};
use clap::{Parser, Subcommand};
use dashbeam_inbox::{
    config::{self, InboxCredential, ReceiverConfig},
    send_files, start_receiver,
};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

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
    let cli = Cli::parse();
    let _transfer_cache = isolate_transfer_cache(&std::env::temp_dir())?;
    match cli.command {
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

fn isolate_transfer_cache(root: &Path) -> Result<tempfile::TempDir> {
    // Desktop startup sweeps .dashbeam-send-* and .dashbeam-recv-* at the temp root.
    let cache = tempfile::Builder::new()
        .prefix("dashbeam-inbox-")
        .tempdir_in(root)?;
    engine::storage::TEMP_DIR
        .set(cache.path().to_path_buf())
        .map_err(|_| anyhow::anyhow!("transfer cache was already initialized"))?;
    Ok(cache)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_startup_cleanup_cannot_remove_cli_transfer_data() {
        let common_temp = tempfile::tempdir().unwrap();
        let cache = isolate_transfer_cache(common_temp.path()).unwrap();
        let store = engine::storage::new_send_blobs_dir();
        assert_eq!(engine::storage::temp_dir(), cache.path());
        assert_eq!(store.parent(), Some(cache.path()));
        std::fs::create_dir_all(&store).unwrap();
        let marker = store.join("active-transfer");
        std::fs::write(&marker, "still in use").unwrap();

        // Reproduce the desktop's top-level cleanup, within this test's private fixture.
        for entry in std::fs::read_dir(common_temp.path()).unwrap() {
            let entry = entry.unwrap();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let swept = name.starts_with(engine::storage::SEND_DIR_PREFIX)
                || name.starts_with(engine::storage::RECV_DIR_PREFIX)
                || engine::storage::LEGACY_DIR_PREFIXES
                    .iter()
                    .any(|prefix| name.starts_with(prefix));
            if swept && entry.path().is_dir() {
                std::fs::remove_dir_all(entry.path()).unwrap();
            }
        }
        assert_eq!(std::fs::read_to_string(marker).unwrap(), "still in use");
    }
}
