use dashbeam_inbox::{
    config::new_credentials,
    send_files, start_receiver,
    wire::{read_message, write_message, Request, Response},
    INBOX_ALPN,
};
use iroh::{endpoint::presets, Endpoint, EndpointAddr};
use std::time::Duration;
use tokio::time::timeout;

const DEADLINE: Duration = Duration::from_secs(30);

async fn direct_address(endpoint: &Endpoint) -> EndpointAddr {
    timeout(DEADLINE, async {
        loop {
            let address = endpoint.addr();
            if address.ip_addrs().next().is_some() {
                return address;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("endpoint did not acquire a direct address")
}

#[tokio::test]
async fn accepts_repeated_uploads_and_keeps_each_in_a_separate_directory() {
    timeout(DEADLINE, async {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        std::fs::create_dir_all(source.join("nested")).unwrap();
        let payload: Vec<u8> = (0..128 * 1024).map(|i| (i % 251) as u8).collect();
        std::fs::write(source.join("nested/data.bin"), &payload).unwrap();
        std::fs::write(source.join("hello.txt"), "hello from sender").unwrap();
        let output = dir.path().join("inbox");
        let (config, credential) = new_credentials();
        let receiver = start_receiver(config, output.clone(), true, DEADLINE)
            .await
            .unwrap();
        let address = direct_address(receiver.endpoint()).await;
        let first = send_files(
            &credential,
            address.clone(),
            vec![source.clone()],
            true,
            DEADLINE,
        )
        .await
        .unwrap();
        let second = send_files(&credential, address, vec![source], true, DEADLINE)
            .await
            .unwrap();
        assert_ne!(first, second);
        for transfer in [first, second] {
            let received = output.join(transfer);
            assert_eq!(
                std::fs::read(received.join("source/nested/data.bin")).unwrap(),
                payload
            );
            assert_eq!(
                std::fs::read_to_string(received.join("source/hello.txt")).unwrap(),
                "hello from sender"
            );
        }
        assert_eq!(std::fs::read_dir(output).unwrap().count(), 2);
        receiver.shutdown().await.unwrap();
    })
    .await
    .expect("upload test timed out");
}

#[tokio::test]
async fn wrong_token_is_rejected_before_reading_local_files_and_receiver_stays_available() {
    timeout(DEADLINE, async {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("inbox");
        let (config, credential) = new_credentials();
        let receiver = start_receiver(config, output.clone(), true, DEADLINE)
            .await
            .unwrap();
        let address = direct_address(receiver.endpoint()).await;
        let mut wrong = credential.clone();
        wrong.token = "0".repeat(64);
        let result = send_files(
            &wrong,
            address.clone(),
            vec![dir.path().join("does-not-exist")],
            true,
            DEADLINE,
        )
        .await;
        assert!(result.unwrap_err().to_string().contains("token"));
        assert_eq!(std::fs::read_dir(&output).unwrap().count(), 0);
        let file = dir.path().join("after-rejection.txt");
        std::fs::write(&file, "still available").unwrap();
        let id = send_files(&credential, address, vec![file], true, DEADLINE)
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(output.join(id).join("after-rejection.txt")).unwrap(),
            "still available"
        );
        receiver.shutdown().await.unwrap();
    })
    .await
    .expect("authorization test timed out");
}

#[tokio::test]
async fn unauthenticated_upload_is_rejected_without_writing_files() {
    timeout(DEADLINE, async {
        let dir = tempfile::tempdir().unwrap();
        let (config, _) = new_credentials();
        let receiver = start_receiver(config, dir.path().to_path_buf(), true, DEADLINE)
            .await
            .unwrap();
        let client = Endpoint::builder(presets::Minimal).bind().await.unwrap();
        let conn = client
            .connect(direct_address(receiver.endpoint()).await, INBOX_ALPN)
            .await
            .unwrap();
        let (mut send, mut recv) = conn.open_bi().await.unwrap();
        write_message(
            &mut send,
            &Request::Upload {
                ticket: "untrusted".into(),
            },
        )
        .await
        .unwrap();
        assert_eq!(
            read_message::<Response>(&mut recv).await.unwrap(),
            Response::Rejected
        );
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
        conn.close(0u32.into(), b"done");
        client.close().await;
        receiver.shutdown().await.unwrap();
    })
    .await
    .expect("unauthenticated upload test timed out");
}

#[tokio::test]
async fn receiver_does_not_offer_file_downloads_or_pairing() {
    timeout(DEADLINE, async {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("private.txt"), "receiver private content").unwrap();
        let (config, _) = new_credentials();
        let receiver = start_receiver(config, dir.path().to_path_buf(), true, DEADLINE)
            .await
            .unwrap();
        let client = Endpoint::builder(presets::Minimal).bind().await.unwrap();
        let address = direct_address(receiver.endpoint()).await;
        for protocol in [iroh_blobs::ALPN, b"altsendme/control/1".as_slice()] {
            assert!(client.connect(address.clone(), protocol).await.is_err());
        }
        assert_eq!(
            std::fs::read_to_string(dir.path().join("private.txt")).unwrap(),
            "receiver private content"
        );
        client.close().await;
        receiver.shutdown().await.unwrap();
    })
    .await
    .expect("reverse transfer test timed out");
}
