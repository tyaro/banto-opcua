//! `ServerConfig::pki_dir` を指定すると、自己署名の鍵と証明書を再起動後も使い回す。

use std::{fs, path::Path};

use banto_opcua_client::OpcUaClient;
use banto_opcua_server::{OpcUaServer, ServerConfig};

const CERT: &str = "own/cert.der";
const KEY: &str = "private/private.pem";

/// Unix では秘密鍵とそのディレクトリが所有者だけのものになっている。
fn assert_private_key_is_owner_only(pki: &Path) {
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
    #[cfg(not(unix))]
    let _ = pki;
}

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

    assert_private_key_is_owner_only(&pki);
}

/// 初回生成の途中で落ちるなどして片方だけ残ったら、組で作り直して起動する。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pki_dir_regenerates_a_keypair_when_one_half_is_missing() {
    let root = tempfile::tempdir().expect("試験用ディレクトリ");
    let pki = root.path().join("pki");
    let (cert, key) = start_and_read(&pki).await;

    // 証明書だけ残っている。
    fs::remove_file(pki.join(KEY)).expect("秘密鍵を消す");
    let (cert_after_lost_key, key_after_lost_key) = start_and_read(&pki).await;
    assert_ne!(cert_after_lost_key, cert, "残った証明書は使わず作り直す");
    assert_ne!(key_after_lost_key, key);
    assert_private_key_is_owner_only(&pki);

    // 秘密鍵だけ残っている。
    fs::remove_file(pki.join(CERT)).expect("証明書を消す");
    let (cert_after_lost_cert, key_after_lost_cert) = start_and_read(&pki).await;
    assert_ne!(
        key_after_lost_cert, key_after_lost_key,
        "残った秘密鍵は使わず作り直す"
    );
    assert_ne!(cert_after_lost_cert, cert_after_lost_key);
    assert_private_key_is_owner_only(&pki);
}

/// 書き込めないなどで組を作れなかったら、鍵なしで動かさず起動を失敗にする。
/// root は権限を無視して書けるので、その環境では条件を作れず確かめない。
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pki_dir_fails_when_the_keypair_cannot_be_created() {
    use std::os::unix::fs::PermissionsExt;

    use banto_opcua_server::Error;

    let root = tempfile::tempdir().expect("試験用ディレクトリ");
    let pki = root.path().join("pki");
    let own = pki.join(CERT).parent().expect("own").to_path_buf();
    fs::create_dir_all(&own).expect("own を作る");
    fs::set_permissions(&own, fs::Permissions::from_mode(0o500)).expect("own を書けなくする");
    if fs::write(own.join("probe"), b"").is_ok() {
        fs::set_permissions(&own, fs::Permissions::from_mode(0o755)).expect("権限を戻す");
        eprintln!("root では書き込みエラーを作れないため確かめない");
        return;
    }

    let result = OpcUaServer::start(ServerConfig::default().with_port(0).with_pki_dir(&pki)).await;
    fs::set_permissions(&own, fs::Permissions::from_mode(0o755)).expect("権限を戻す");
    match result {
        Err(Error::Pki(error)) => assert_eq!(error.kind(), std::io::ErrorKind::NotFound),
        Err(other) => panic!("証明書と秘密鍵を作れなかったことを返す: {other}"),
        Ok(server) => {
            server.shutdown().await.expect("停止する");
            panic!("組を作れないまま起動してはいけない");
        }
    }
}
