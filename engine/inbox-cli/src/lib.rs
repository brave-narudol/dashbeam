pub mod config;
pub mod wire;

use anyhow::{bail, ensure, Context, Result};
use config::{InboxCredential, ReceiverConfig};
use engine::{AddrInfoOptions, ReceiveOptions, RelayModeOption, SendOptions};
use iroh::{
    address_lookup::pkarr::PkarrPublisher,
    endpoint::{presets, Connection},
    protocol::{AcceptError, ProtocolHandler, Router},
    Endpoint, EndpointAddr,
};
use iroh_blobs::ticket::BlobTicket;
use std::{fmt, path::PathBuf, sync::Arc, time::Duration};
use tokio::{
    sync::{Mutex, Semaphore},
    time::timeout,
};
use wire::{read_message, write_message, Request, Response};

pub const INBOX_ALPN: &[u8] = b"dashbeam/inbox/1";
const AUTH_TIMEOUT: Duration = Duration::from_secs(15);

struct InboxHandler {
    config: ReceiverConfig,
    output: PathBuf,
    sessions: Semaphore,
    download: Mutex<()>,
    transfer_timeout: Duration,
    direct: bool,
}

impl fmt::Debug for InboxHandler {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InboxHandler").finish_non_exhaustive()
    }
}

impl InboxHandler {
    async fn handle(&self, connection: &Connection) -> Result<()> {
        let (mut send, mut recv) = timeout(AUTH_TIMEOUT, connection.accept_bi()).await??;
        let request: Request = timeout(AUTH_TIMEOUT, read_message(&mut recv)).await??;
        let authenticated = match request {
            Request::Authenticate { token } => self.config.accepts(&token),
            _ => false,
        };
        if !authenticated {
            finish_response(&mut send, &Response::Rejected).await?;
            return Ok(());
        }
        // The engine's partial stores are keyed by content; serialize downloads to avoid collisions.
        let Ok(_download) = self.download.try_lock() else {
            finish_response(&mut send, &Response::Busy).await?;
            return Ok(());
        };
        write_message(&mut send, &Response::Authorized).await?;
        let request: Request = timeout(self.transfer_timeout, read_message(&mut recv)).await??;
        let Request::Upload { ticket } = request else {
            finish_response(&mut send, &Response::Rejected).await?;
            return Ok(());
        };
        let parsed: BlobTicket = ticket.parse().context("invalid upload ticket")?;
        ensure!(
            parsed.format() == iroh_blobs::BlobFormat::HashSeq,
            "expected a file collection"
        );
        let result = self.receive(connection, ticket).await;
        let response = match result {
            Ok(transfer_id) => {
                println!("Received {transfer_id}");
                Response::Complete { transfer_id }
            }
            Err(error) => {
                eprintln!("Upload failed: {error:#}");
                Response::Failed
            }
        };
        finish_response(&mut send, &response).await
    }

    async fn receive(&self, connection: &Connection, ticket: String) -> Result<String> {
        // A fresh directory prevents overwrites and traversal through pre-existing symlinks.
        let staging = tempfile::Builder::new()
            .prefix(".receiving-")
            .tempdir_in(&self.output)?;
        let options = ReceiveOptions {
            output_dir: Some(staging.path().to_path_buf()),
            relay_mode: relay_mode(self.direct),
            ..Default::default()
        };
        let (cancel_tx, cancel_rx) = tokio::sync::oneshot::channel();
        let download = engine::download(ticket, options, None, cancel_rx);
        tokio::pin!(download);
        tokio::select! {
            result = &mut download => { result?; }
            _ = connection.closed() => {
                let _ = cancel_tx.send(());
                let _ = download.await;
                bail!("sender disconnected");
            }
            _ = tokio::time::sleep(self.transfer_timeout) => {
                let _ = cancel_tx.send(());
                let _ = download.await;
                bail!("transfer timed out");
            }
        }
        let transfer_id = uuid::Uuid::new_v4().to_string();
        std::fs::rename(staging.path(), self.output.join(&transfer_id))?;
        Ok(transfer_id)
    }
}

impl ProtocolHandler for InboxHandler {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let Ok(_slot) = self.sessions.try_acquire() else {
            connection.close(429u32.into(), b"busy");
            return Ok(());
        };
        // Tokens and untrusted message bodies are never included in diagnostics.
        if self.handle(&connection).await.is_err() {
            eprintln!("Inbox session rejected or interrupted");
        }
        connection.close(0u32.into(), b"finished");
        Ok(())
    }
}

async fn finish_response(send: &mut iroh::endpoint::SendStream, response: &Response) -> Result<()> {
    write_message(send, response).await?;
    send.finish()?;
    let _ = timeout(AUTH_TIMEOUT, send.stopped()).await;
    Ok(())
}

fn relay_mode(direct: bool) -> RelayModeOption {
    if direct {
        RelayModeOption::Disabled
    } else {
        RelayModeOption::Default
    }
}

pub async fn start_receiver(
    config: ReceiverConfig,
    output: PathBuf,
    direct: bool,
    transfer_timeout: Duration,
) -> Result<Router> {
    ensure!(!transfer_timeout.is_zero(), "timeout must be positive");
    let secret = config.secret_key()?;
    std::fs::create_dir_all(&output)?;
    let output = output.canonicalize()?;
    let builder = if direct {
        Endpoint::builder(presets::Minimal)
    } else {
        Endpoint::builder(presets::N0).address_lookup(PkarrPublisher::n0_dns())
    };
    let endpoint = builder
        .secret_key(secret)
        .alpns(vec![INBOX_ALPN.to_vec()])
        .bind()
        .await?;
    let handler = InboxHandler {
        config,
        output,
        sessions: Semaphore::new(32),
        download: Mutex::new(()),
        transfer_timeout,
        direct,
    };
    // Register only uploads: never a blob provider, metadata server, pairing, or presence service.
    let router = Router::builder(endpoint)
        .accept(INBOX_ALPN, Arc::new(handler))
        .spawn();
    if !direct {
        if timeout(Duration::from_secs(30), router.endpoint().online())
            .await
            .is_err()
        {
            router.shutdown().await?;
            bail!("receiver could not reach a relay");
        }
    }
    Ok(router)
}

pub async fn send_files(
    credential: &InboxCredential,
    target: EndpointAddr,
    paths: Vec<PathBuf>,
    direct: bool,
    transfer_timeout: Duration,
) -> Result<String> {
    credential.validate()?;
    ensure!(
        target.id.to_string() == credential.endpoint_id,
        "target does not match inbox identity"
    );
    ensure!(!paths.is_empty(), "select at least one file or directory");
    ensure!(!transfer_timeout.is_zero(), "timeout must be positive");
    let builder = if direct {
        Endpoint::builder(presets::Minimal)
    } else {
        Endpoint::builder(presets::N0)
    };
    let endpoint = builder.alpns(vec![]).bind().await?;
    let result = timeout(
        transfer_timeout,
        send_session(&endpoint, target, credential, paths, direct),
    )
    .await;
    endpoint.close().await;
    result.context("send timed out")?
}

async fn send_session(
    endpoint: &Endpoint,
    target: EndpointAddr,
    credential: &InboxCredential,
    paths: Vec<PathBuf>,
    direct: bool,
) -> Result<String> {
    let connection = timeout(
        Duration::from_secs(30),
        endpoint.connect(target, INBOX_ALPN),
    )
    .await??;
    let (mut send, mut recv) = connection.open_bi().await?;
    write_message(
        &mut send,
        &Request::Authenticate {
            token: credential.token.clone(),
        },
    )
    .await?;
    match timeout(AUTH_TIMEOUT, read_message::<Response>(&mut recv)).await?? {
        Response::Authorized => {}
        Response::Busy => bail!("receiver is busy; retry after the current upload"),
        _ => bail!("receiver rejected the upload token"),
    }
    let share = engine::start_share_items(
        paths,
        SendOptions {
            relay_mode: relay_mode(direct),
            ticket_type: AddrInfoOptions::RelayAndAddresses,
            ..Default::default()
        },
        &None,
        None,
    )
    .await?;
    let result = async {
        write_message(
            &mut send,
            &Request::Upload {
                ticket: share.ticket.clone(),
            },
        )
        .await?;
        send.finish()?;
        match read_message::<Response>(&mut recv).await? {
            Response::Complete { transfer_id } => Ok(transfer_id),
            _ => bail!("receiver could not finish the upload"),
        }
    }
    .await;
    share.router.shutdown().await?;
    connection.close(0u32.into(), b"finished");
    result
}
