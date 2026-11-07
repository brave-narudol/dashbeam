use anyhow::{ensure, Context, Result};
use data_encoding::HEXLOWER;
use iroh::SecretKey;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::{
    fs::OpenOptions,
    io::{Read, Write},
    path::Path,
};
use subtle::ConstantTimeEq;

const MAX_CONFIG: u64 = 16 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReceiverConfig {
    version: u8,
    secret_key: [u8; 32],
    token_hash: [u8; 32],
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InboxCredential {
    version: u8,
    pub endpoint_id: String,
    pub token: String,
}

pub fn new_credentials() -> (ReceiverConfig, InboxCredential) {
    let secret = SecretKey::generate();
    let token = HEXLOWER.encode(&rand::random::<[u8; 32]>());
    let config = ReceiverConfig {
        version: 1,
        secret_key: secret.to_bytes(),
        token_hash: *blake3::hash(token.as_bytes()).as_bytes(),
    };
    let credential = InboxCredential {
        version: 1,
        endpoint_id: secret.public().to_string(),
        token,
    };
    (config, credential)
}

impl ReceiverConfig {
    pub fn secret_key(&self) -> Result<SecretKey> {
        ensure!(self.version == 1, "unsupported receiver config version");
        Ok(SecretKey::from_bytes(&self.secret_key))
    }

    pub fn accepts(&self, token: &str) -> bool {
        let actual = blake3::hash(token.as_bytes());
        self.version == 1
            && token.len() == 64
            && bool::from(self.token_hash.ct_eq(actual.as_bytes()))
    }
}

impl InboxCredential {
    pub fn validate(&self) -> Result<()> {
        ensure!(self.version == 1, "unsupported inbox credential version");
        self.endpoint_id
            .parse::<iroh::EndpointId>()
            .context("invalid inbox identity")?;
        ensure!(
            self.token.len() == 64 && HEXLOWER.decode(self.token.as_bytes()).is_ok(),
            "invalid inbox token"
        );
        Ok(())
    }
}

pub fn read_config<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let file = std::fs::File::open(path).context("open config file")?;
    let mut bytes = Vec::new();
    file.take(MAX_CONFIG + 1).read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= MAX_CONFIG as usize, "config file too large");
    serde_json::from_slice(&bytes).context("invalid config file")
}

// create_new avoids replacing credentials; Unix credentials are owner-only from creation.
pub fn write_config<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .context("create config (existing files are never overwritten)")?;
    file.write_all(&serde_json::to_vec_pretty(value)?)?;
    file.sync_all()?;
    Ok(())
}

pub fn initialize(config_path: &Path, credential_path: &Path) -> Result<()> {
    ensure!(
        config_path != credential_path,
        "use separate receiver and sender config paths"
    );
    ensure!(
        !config_path.exists() && !credential_path.exists(),
        "config already exists"
    );
    let (config, credential) = new_credentials();
    write_config(config_path, &config)?;
    write_config(credential_path, &credential)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_issued_token_is_accepted() {
        let (config, credential) = new_credentials();
        assert!(config.accepts(&credential.token));
        assert!(!config.accepts(&"0".repeat(64)));
        assert!(!config.accepts(""));
        let (_, other) = new_credentials();
        assert!(!config.accepts(&other.token));
        credential.validate().unwrap();
    }

    #[test]
    fn credentials_preserve_identity_without_exposing_receiver_key() {
        let (config, credential) = new_credentials();
        let encoded = serde_json::to_vec(&config).unwrap();
        let decoded: ReceiverConfig = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(
            decoded.secret_key().unwrap().public().to_string(),
            credential.endpoint_id
        );
        assert!(decoded.accepts(&credential.token));
        let shared = serde_json::to_value(credential).unwrap();
        assert!(shared.get("secret_key").is_none());
    }

    #[test]
    fn init_refuses_to_replace_existing_credentials() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("receiver.json");
        let credential = dir.path().join("sender.json");
        initialize(&config, &credential).unwrap();
        let original = std::fs::read(&config).unwrap();
        assert!(initialize(&config, &credential).is_err());
        assert_eq!(std::fs::read(&config).unwrap(), original);
    }
}
