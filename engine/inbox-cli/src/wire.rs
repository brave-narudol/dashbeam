use anyhow::{ensure, Context, Result};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

const MAX_MESSAGE: usize = 8192;

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Request {
    Authenticate { token: String },
    Upload { ticket: String },
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Response {
    Authorized,
    Rejected,
    Busy,
    Complete { transfer_id: String },
    Failed,
}

pub async fn write_message<T: Serialize>(
    writer: &mut (impl AsyncWrite + Unpin),
    value: &T,
) -> Result<()> {
    let bytes = serde_json::to_vec(value)?;
    ensure!(bytes.len() <= MAX_MESSAGE, "message too large");
    writer.write_u32(bytes.len() as u32).await?;
    writer.write_all(&bytes).await?;
    writer.flush().await?;
    Ok(())
}

pub async fn read_message<T: DeserializeOwned>(reader: &mut (impl AsyncRead + Unpin)) -> Result<T> {
    let size = reader.read_u32().await? as usize;
    ensure!(size <= MAX_MESSAGE, "message too large");
    let mut bytes = vec![0; size];
    reader.read_exact(&mut bytes).await?;
    serde_json::from_slice(&bytes).context("invalid inbox message")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn oversized_messages_are_rejected_before_reading_the_body() {
        let bytes = ((MAX_MESSAGE + 1) as u32).to_be_bytes();
        assert!(read_message::<Request>(&mut bytes.as_slice())
            .await
            .is_err());
    }

    #[test]
    fn there_is_no_download_or_list_request() {
        for input in [
            r#"{"type":"download","path":"secret.txt"}"#,
            r#"{"type":"list"}"#,
        ] {
            assert!(serde_json::from_str::<Request>(input).is_err());
        }
    }
}
