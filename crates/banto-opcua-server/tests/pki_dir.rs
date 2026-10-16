//! `ServerConfig::pki_dir` を指定すると、自己署名の鍵と証明書を再起動後も使い回す。

use std::{fs, path::Path};

use banto_opcua_client::OpcUaClient;
use banto_opcua_server::{OpcUaServer, ServerConfig};

const CERT: &str = "own/cert.der";
const KEY: &str = "private/private.pem";

async fn start_and_read(pki: &Path) -> (Vec<u8>, Vec<u8>) {
    let server = OpcUaServer::start(ServerConfig::default().with_port(0).with_pki_dir(pki))
        .await
        .expect("pki_dir を指定して起動する");
    let client = OpcUaClient::connect(server.endpoint())
        .await
        .expect("鍵を読み直したサーバーへ接続する");
    assert_eq!(
        client.read_server_state().await.expect("状態を読む"),
        0,
        "ServerState::Running"
    );
    drop(client);
    server.shutdown().await.expect("停止する");
    (
        fs::read(pki.join(CERT)).expect("証明書が残る"),
        fs::read(pki.join(KEY)).expect("秘密鍵が残る"),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pki_dir_reuses_the_generated_keypair_across_restarts() {
    let root = tempfile::tempdir().expect("試験用ディレクトリ");
    // 実運用に近く、親ディレクトリは他のユーザーも辿れる状態にする。
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o755)).expect("親の権限");
    }
    // 無いディレクトリも作る。
    let pki = root.path().join("opcua").join("pki");

    let first = start_and_read(&pki).await;
    let second = start_and_read(&pki).await;
    assert_eq!(first, second, "2 回目の起動は鍵と証明書を作り直さない");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let mode =
            |path: &Path| fs::metadata(path).expect("権限を読む").permissions().mode() & 0o777;
        assert_eq!(mode(&pki.join(KEY)), 0o600, "秘密鍵は所有者だけが読める");
        assert_eq!(
            mode(pki.join(KEY).parent().expect("private")),
            0o700,
            "秘密鍵のディレクトリは所有者だけが開ける"
        );
    }
}
