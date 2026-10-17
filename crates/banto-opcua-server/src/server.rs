//! async-opcua の `ServerBuilder` を、ループバック既定の待受で包む。
//!
//! Phase 1 の固定タグと、追加・削除できるタグを Objects 以下に公開する。

use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
    time::SystemTime,
};

use opcua::{
    server::{ANONYMOUS_USER_TOKEN_ID, ServerBuilder, ServerEndpoint, ServerHandle},
    types::{DataValue, Variant},
};
use tokio::{net::TcpListener, task::JoinHandle};

use banto_opcua_core::{DataType, MappedValue, Quality, SampleTimestamps};

use crate::{
    Error, ServerConfig, TagReading, WriteHandler,
    config::{
        host_is_loopback, host_is_unspecified, normalize_endpoint_path, valid_advertised_host,
    },
    fixed_tags::{self, FixedTags},
    tag_manager::WritePath,
};

/// サーバーを起動する入口。状態は持たない。
#[derive(Debug, Default)]
pub struct OpcUaServer;

impl OpcUaServer {
    /// 設定のアドレスで待受を始める。ポート `0` のときは空きポートを使う。
    ///
    /// 返した [`RunningServer`] を落とすと待受タスクを中止する。タスクの終了を
    /// 待つときは [`RunningServer::shutdown`]。
    ///
    /// 未指定アドレス（`0.0.0.0` / `::`）で、`advertised_host` が無いときは
    /// bind せずに [`Error::AdvertisedHostRequired`] を返す。ループバック以外へ
    /// 公開するときは警告ログを出す。
    ///
    /// `pki_dir` を作れないとき、証明書と秘密鍵の組を作れなかったとき、または
    /// Unix で残す秘密鍵の権限を絞れないときは [`Error::Pki`] を返す。
    ///
    /// 書き込みハンドラーを渡さないので、すべてのタグは読み取り専用になる。
    pub async fn start(config: ServerConfig) -> Result<RunningServer, Error> {
        Self::start_with(config, None).await
    }

    /// 書き込みハンドラーを渡して起動する（ADR-0004）。
    ///
    /// [`RunningServer::set_tag_writable`] で書き込み可にしたタグへの Write を、
    /// サーバーの検査を通したうえで `handler` へ渡す。Anonymous で書けるので、
    /// ループバック以外へ公開するときは警告ログを出す。ほかは [`Self::start`] と同じ。
    pub async fn start_with_write_handler(
        config: ServerConfig,
        handler: Arc<dyn WriteHandler>,
    ) -> Result<RunningServer, Error> {
        Self::start_with(config, Some(handler)).await
    }

    async fn start_with(
        mut config: ServerConfig,
        handler: Option<Arc<dyn WriteHandler>>,
    ) -> Result<RunningServer, Error> {
        if config.namespace_uri.trim().is_empty()
            || config.namespace_uri != config.namespace_uri.trim()
            || config.namespace_uri == "http://opcfoundation.org/UA/"
            || config.namespace_uri == config.application_uri
        {
            return Err(Error::InvalidNamespaceUri(config.namespace_uri));
        }
        if config.bind.is_unspecified() && config.explicit_advertised_host().is_none() {
            return Err(Error::AdvertisedHostRequired);
        }
        if config
            .explicit_advertised_host()
            .is_some_and(host_is_unspecified)
        {
            return Err(Error::AdvertisedHostRequired);
        }
        if let Some(host) = config.advertised_host.as_deref()
            && !host.trim().is_empty()
            && !valid_advertised_host(host)
        {
            return Err(Error::InvalidAdvertisedHost(host.to_owned()));
        }

        let pki = Pki::prepare(config.pki_dir.take())?;
        let listener = TcpListener::bind((config.bind, config.port))
            .await
            .map_err(Error::Bind)?;
        let local_addr = listener.local_addr().map_err(Error::Bind)?;
        let path = normalize_endpoint_path(&config.endpoint_path);
        let endpoint_host = config.endpoint_host(local_addr);
        let endpoint = config.endpoint_url(local_addr);
        if !config.bind.is_loopback() || !host_is_loopback(&endpoint_host) {
            tracing::warn!(
                bind = %config.bind,
                advertised_host = %endpoint_host,
                "OPC UA サーバーをループバック以外へ公開します。Phase 1 は SecurityPolicy None / Anonymous です"
            );
            if handler.is_some() {
                tracing::warn!(
                    bind = %config.bind,
                    advertised_host = %endpoint_host,
                    "OPC UA の書き込みを受け付けます。同じネットワークから Anonymous で書けます"
                );
            }
        }

        let namespace_uri = config.namespace_uri.clone();
        let mut builder = ServerBuilder::new()
            .application_name(config.application_name)
            .application_uri(config.application_uri)
            .product_uri(config.product_uri)
            .create_sample_keypair(true)
            .certificate_path(CERTIFICATE_PATH)
            .private_key_path(PRIVATE_KEY_PATH)
            .pki_dir(pki.path())
            .host(endpoint_host)
            .port(local_addr.port())
            .trust_client_certs(true)
            .add_endpoint(
                "none",
                ServerEndpoint::new_none(&path, &[ANONYMOUS_USER_TOKEN_ID.to_owned()]),
            )
            .discovery_urls(vec![endpoint.clone()])
            .with_node_manager(fixed_tags::node_manager(
                namespace_uri.clone(),
                handler.map(|handler| WritePath::new(handler, config.max_concurrent_writes)),
            ));
        // async-opcua の既定値 100 ms は 0 要求も補正する。厳密な採取時刻が
        // 必要なクライアントには、実際に 0 ms の監視項目を作れるようにする。
        builder.limits_mut().subscriptions.min_sampling_interval_ms = 0.0;
        builder.limits_mut().operational.max_nodes_per_write = config.max_nodes_per_write.get();
        let (server, handle) = builder.build().map_err(Error::Build)?;
        pki.finish()?;
        let fixed_tags = FixedTags::register(&handle, &namespace_uri, config.sample_tags)?;

        // 一時ディレクトリはタスクが所有する。Drop が先に消すと、停止前の
        // サーバーが証明書を読めなくなる。`pki_dir` は呼び出し側が所有し、消さない。
        let task = tokio::spawn(async move {
            let _pki = pki;
            server.run_with(listener).await
        });

        Ok(RunningServer {
            local_addr,
            endpoint,
            handle,
            fixed_tags,
            task: Some(task),
        })
    }
}

const CERTIFICATE_PATH: &str = "own/cert.der";
const PRIVATE_KEY_PATH: &str = "private/private.pem";

/// 証明書と秘密鍵の片方だけが残っていたら消し、async-opcua に組で作り直させる。
///
/// async-opcua は既存のファイルを上書きしないので、片方だけ残ると、新しい証明書と
/// 古い鍵の組み違いが残るか、鍵の無いまま起動する。初回生成の途中でプロセスが
/// 落ちたときに起こり得る。両方あれば中身は検証しない（壊れていたら両方消す）。
fn discard_unpaired_keypair(dir: &Path) -> Result<(), Error> {
    let cert = dir.join(CERTIFICATE_PATH);
    let key = dir.join(PRIVATE_KEY_PATH);
    // 権限エラーなどを「無い」と取り違えて、残っている片方を消さない。
    let exists = |path: &Path| path.try_exists().map_err(Error::Pki);
    let lone = match (exists(&cert)?, exists(&key)?) {
        (true, false) => cert,
        (false, true) => key,
        _ => return Ok(()),
    };
    tracing::warn!(
        path = %lone.display(),
        "証明書と秘密鍵の片方だけが残っているため消して作り直します"
    );
    std::fs::remove_file(&lone).map_err(Error::Pki)
}

/// 自己署名証明書の置き場。一時ディレクトリは停止とともに消え、
/// `pki_dir` は次の起動で同じ鍵を読み直すために残す。
enum Pki {
    Temporary(tempfile::TempDir),
    Persistent(PathBuf),
}

impl Pki {
    fn prepare(dir: Option<PathBuf>) -> Result<Self, Error> {
        match dir {
            Some(dir) => {
                std::fs::create_dir_all(&dir).map_err(Error::Pki)?;
                protect_private_dir(&dir.join(PRIVATE_KEY_PATH))?;
                discard_unpaired_keypair(&dir)?;
                Ok(Self::Persistent(dir))
            }
            None => tempfile::tempdir().map(Self::Temporary).map_err(Error::Pki),
        }
    }

    fn path(&self) -> &Path {
        match self {
            Self::Temporary(dir) => dir.path(),
            Self::Persistent(dir) => dir,
        }
    }

    /// `pki_dir` に証明書と秘密鍵の組がそろったことを確かめ、秘密鍵を絞る。
    ///
    /// async-opcua は生成に失敗しても（書き込めない、容量不足など）鍵なしで起動を
    /// 続ける。`pki_dir` を指定したのに組が無いまま動くと、次の起動でも重い生成を
    /// 繰り返すので、ここで [`Error::Pki`] にする。片方だけ残ったファイルは次の
    /// 起動の [`discard_unpaired_keypair`] が片付ける。一時ディレクトリは
    /// tempfile が所有者だけに作り、停止で消えるので触らない。
    fn finish(&self) -> Result<(), Error> {
        let Self::Persistent(dir) = self else {
            return Ok(());
        };
        for path in [dir.join(CERTIFICATE_PATH), dir.join(PRIVATE_KEY_PATH)] {
            if !path.try_exists().map_err(Error::Pki)? {
                return Err(Error::Pki(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!("証明書と秘密鍵を作れませんでした: {}", path.display()),
                )));
            }
        }
        restrict_private_key(&dir.join(PRIVATE_KEY_PATH))
    }
}

/// 秘密鍵自体も所有者だけが読めるようにする。async-opcua は umask の権限で書くが、
/// 親ディレクトリは [`protect_private_dir`] が生成前に 0700 にしているので、
/// ここまでの間も他のユーザーは辿れない。
#[cfg(unix)]
fn restrict_private_key(key: &Path) -> Result<(), Error> {
    use std::os::unix::fs::PermissionsExt;

    std::fs::set_permissions(key, std::fs::Permissions::from_mode(0o600)).map_err(Error::Pki)
}

#[cfg(not(unix))]
fn restrict_private_key(_key: &Path) -> Result<(), Error> {
    Ok(())
}

/// 秘密鍵のディレクトリを、鍵を生成・読み込みする前に所有者だけのものにする。
/// 無ければ 0700 で作り、既にあれば権限を 0700 に絞る。
#[cfg(unix)]
fn protect_private_dir(key: &Path) -> Result<(), Error> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};

    let Some(dir) = key.parent() else {
        return Ok(());
    };
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .map_err(Error::Pki)?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).map_err(Error::Pki)
}

#[cfg(not(unix))]
fn protect_private_dir(_key: &Path) -> Result<(), Error> {
    Ok(())
}

/// 起動済みのサーバー。
///
/// [`Drop`] はキャンセルと `abort` だけを行い、停止完了は待たない。
/// サンプル証明書の一時ディレクトリは待受タスクが所有するので、タスクが
/// 終わるまで削除されない。停止完了を待つときは [`Self::shutdown`]。
#[must_use = "シャットダウンするまで RunningServer を保持する"]
pub struct RunningServer {
    local_addr: SocketAddr,
    endpoint: String,
    handle: ServerHandle,
    fixed_tags: FixedTags,
    task: Option<JoinHandle<Result<(), String>>>,
}

impl RunningServer {
    /// 実際に bind したソケット。
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// クライアントが接続する `opc.tcp://` URL。
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// タグの名前空間インデックス。クライアントは URI からも取得できる。
    pub fn namespace_index(&self) -> u16 {
        self.fixed_tags.namespace_index
    }

    /// Phase 1 の固定タグの名前空間インデックス。
    pub fn fixed_namespace_index(&self) -> u16 {
        self.namespace_index()
    }

    /// `{connection}.{group}.{tag}` のタグを追加する。書き込み不可で登録する。
    /// 同じ NodeId が既にあればエラー。接続・グループのフォルダーは必要なら作る。
    /// 各名前は空文字にできない。ドットと接続名 `Banto` は使用できる。
    /// 初期値には登録時刻を付ける試験用 API。
    /// ハブの実データと SourceTimestamp の結合は banto-industrial #201 で行う。
    pub fn add_tag(
        &self,
        connection: &str,
        group: &str,
        tag: &str,
        value: MappedValue,
    ) -> Result<(), Error> {
        let value = match value {
            MappedValue::Boolean(bit) => Variant::Boolean(bit),
            MappedValue::Double(number) if number.is_finite() => Variant::Double(number),
            MappedValue::Double(_) => {
                return Err(Error::TagUpdate(
                    "タグの初期値は有限値にしてください".to_owned(),
                ));
            }
        };
        self.fixed_tags
            .add_tag(&self.handle, connection, group, tag, value)
    }

    /// 実データの型・品質・時刻を保持したまま、タグを書き込み不可で登録する。
    /// 値が未取得でも DataType は `reading.data_type` で確定する。
    pub fn add_tag_reading(
        &self,
        connection: &str,
        group: &str,
        tag: &str,
        reading: TagReading,
    ) -> Result<(), Error> {
        self.fixed_tags.add_tag_reading(
            &self.handle,
            connection,
            group,
            tag,
            reading.data_type,
            reading.data_value(),
        )
    }

    /// 登録済みタグの値・品質・時刻を更新し、購読者へ変更を通知する。
    /// DataType の変更は同じ NodeId への更新ではなく削除・再登録で行う。
    pub fn update_tag_reading(
        &self,
        connection: &str,
        group: &str,
        tag: &str,
        reading: TagReading,
    ) -> Result<(), Error> {
        self.fixed_tags.update_tag_reading(
            &self.handle,
            connection,
            group,
            tag,
            reading.data_type,
            reading.data_value(),
        )
    }

    /// 登録済みのタグを書き込み可・不可に切り替える（ADR-0004）。
    ///
    /// 書き込み可のタグだけ、AccessLevel と UserAccessLevel に CurrentWrite を
    /// 付ける。書き込みハンドラーなしで起動したサーバーでは、`writable` でも
    /// 書き込み不可のままにする。サンプルタグは切り替えられない。削除して
    /// 再登録したタグは、書き込み不可から始まる。
    pub fn set_tag_writable(
        &self,
        connection: &str,
        group: &str,
        tag: &str,
        writable: bool,
    ) -> Result<(), Error> {
        self.fixed_tags
            .set_tag_writable(connection, group, tag, writable)
    }

    /// タグを削除し、既存の購読へ `BadNodeIdUnknown` を通知する。
    /// MonitoredItem は保持し、同じ NodeId の再登録で通知が再開する。
    ///
    /// 空になった接続・グループのフォルダーは消さず、Browse に残る。削除と
    /// 再登録の間も、クライアントが控えたフォルダーの NodeId を無効にしないため。
    /// 残るフォルダーの数は、このプロセスで使った接続と（接続, グループ）の組の
    /// 数までで、アドレス空間はメモリ上にしかないので再起動すれば消える。
    pub fn remove_tag(&self, connection: &str, group: &str, tag: &str) -> Result<(), Error> {
        self.fixed_tags
            .remove_tag(&self.handle, connection, group, tag)
    }

    /// Phase 1 の固定タグ `PLC1.Line1.Speed` を更新して購読者へ通知する。
    /// 有限値以外は拒否する。
    /// ハブのタグレジストリを接続する Phase 2 で更新 API を置き換える。
    pub fn set_fixed_speed(&self, speed: f64) -> Result<(), Error> {
        if !speed.is_finite() {
            return Err(Error::TagUpdate("Speed は有限値にしてください".to_owned()));
        }
        self.fixed_tags
            .update(&self.handle, "Speed", DataValue::new_now(speed))
    }

    /// 固定タグ `Speed` の現在値と品質を更新する。Phase 2 の結合前に
    /// フィルタ未指定の購読通知を検証するための入口。
    /// Bad 品質と欠損値では、core の写像に従い値を公開しない。
    /// 実タグの時刻はここで現在時刻を作らず、#8 の SampleTimestamps を使う。
    pub fn set_fixed_speed_reading(
        &self,
        speed: Option<f64>,
        quality: Quality,
    ) -> Result<(), Error> {
        let now = SystemTime::now();
        let reading = TagReading::new(
            DataType::Double,
            speed,
            quality,
            SampleTimestamps::new(Some(now), now),
        );
        self.fixed_tags
            .update(&self.handle, "Speed", reading.data_value())
    }

    /// Phase 1 の固定タグ `PLC1.Line1.Running` を更新して購読者へ通知する。
    /// ハブのタグレジストリを接続する Phase 2 で更新 API を置き換える。
    pub fn set_fixed_running(&self, running: bool) -> Result<(), Error> {
        self.fixed_tags
            .update(&self.handle, "Running", DataValue::new_now(running))
    }

    /// クライアントへ切断を知らせてからタスクの終了を待つ。
    ///
    /// [`Drop`] と違い、待受が戻るまで待つ。一時ディレクトリの削除はその後。
    pub async fn shutdown(mut self) -> Result<(), Error> {
        self.handle.cancel();
        let Some(task) = self.task.take() else {
            return Ok(());
        };
        match task.await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(message)) => Err(Error::Runtime(message)),
            Err(err) => Err(Error::Runtime(err.to_string())),
        }
    }
}

impl std::fmt::Debug for RunningServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunningServer")
            .field("local_addr", &self.local_addr)
            .field("endpoint", &self.endpoint)
            .finish_non_exhaustive()
    }
}

impl Drop for RunningServer {
    fn drop(&mut self) {
        self.handle.cancel();
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        net::{IpAddr, Ipv4Addr},
        sync::mpsc,
        time::Duration,
    };

    use opcua::{
        server::address_space::NodeType,
        types::{DataEncoding, NodeId, NumericRange, TimestampsToReturn},
    };

    use super::*;

    /// 存在を確かめられないときは「無い」と扱わず、残っている鍵を消さない。
    /// root は権限を無視して読めるので、その環境では条件を作れず確かめない。
    #[cfg(unix)]
    #[test]
    fn unreadable_certificate_dir_keeps_the_keypair() {
        use std::os::unix::fs::PermissionsExt;

        let root = tempfile::tempdir().expect("試験用ディレクトリ");
        let pki = root.path().join("pki");
        let cert = pki.join(CERTIFICATE_PATH);
        let key = pki.join(PRIVATE_KEY_PATH);
        for path in [&cert, &key] {
            std::fs::create_dir_all(path.parent().expect("親")).expect("親を作る");
            std::fs::write(path, b"keep").expect("ファイルを置く");
        }
        let own = cert.parent().expect("own");
        std::fs::set_permissions(own, std::fs::Permissions::from_mode(0o000))
            .expect("own を開けなくする");
        if cert.try_exists().is_ok() {
            std::fs::set_permissions(own, std::fs::Permissions::from_mode(0o755))
                .expect("権限を戻す");
            eprintln!("root では権限エラーを作れないため確かめない");
            return;
        }

        let result = Pki::prepare(Some(pki.clone()));
        std::fs::set_permissions(own, std::fs::Permissions::from_mode(0o755)).expect("権限を戻す");
        assert!(
            matches!(&result, Err(Error::Pki(error)) if error.kind() == std::io::ErrorKind::PermissionDenied),
            "確かめられないときは権限エラーで起動しない: {:?}",
            result.as_ref().err()
        );
        assert_eq!(std::fs::read(&key).expect("秘密鍵が残る"), b"keep");
        assert_eq!(std::fs::read(&cert).expect("証明書が残る"), b"keep");
    }

    /// 鍵の生成前（`ServerBuilder::build` の前）に、秘密鍵のディレクトリが
    /// 所有者だけのものになっている。既存の緩いディレクトリも絞る。
    #[cfg(unix)]
    #[test]
    fn persistent_pki_protects_the_private_dir_before_key_generation() {
        use std::os::unix::fs::PermissionsExt;

        let mode = |path: &Path| {
            std::fs::metadata(path)
                .expect("権限を読む")
                .permissions()
                .mode()
                & 0o777
        };
        let root = tempfile::tempdir().expect("試験用ディレクトリ");
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o755))
            .expect("親を他のユーザーが辿れる状態にする");

        let fresh = root.path().join("fresh");
        let pki = Pki::prepare(Some(fresh.clone())).expect("新しい保存先を用意する");
        assert_eq!(mode(&fresh.join("private")), 0o700);
        assert!(
            !fresh.join(PRIVATE_KEY_PATH).exists(),
            "鍵はまだ無い（生成前に保護済み）"
        );
        drop(pki);

        let loose = root.path().join("loose");
        std::fs::create_dir_all(loose.join("private")).expect("既存の private");
        std::fs::set_permissions(
            loose.join("private"),
            std::fs::Permissions::from_mode(0o755),
        )
        .expect("緩い権限にする");
        Pki::prepare(Some(loose.clone())).expect("既存の保存先を用意する");
        assert_eq!(mode(&loose.join("private")), 0o700);
    }

    #[tokio::test]
    async fn unspecified_bind_without_advertised_host_is_rejected() {
        let err = OpcUaServer::start(ServerConfig {
            bind: IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            port: 0,
            ..ServerConfig::default()
        })
        .await
        .expect_err("advertised_host が必要");
        assert!(matches!(err, Error::AdvertisedHostRequired));
    }

    #[tokio::test]
    async fn advertising_an_unspecified_host_is_rejected() {
        let err = OpcUaServer::start(ServerConfig {
            advertised_host: Some("0.0.0.0".to_owned()),
            port: 0,
            ..ServerConfig::default()
        })
        .await
        .expect_err("0.0.0.0 は公開ホストにできない");
        assert!(matches!(err, Error::AdvertisedHostRequired));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn advertised_host_replaces_the_bound_address_in_the_endpoint() {
        let server = OpcUaServer::start(ServerConfig {
            bind: IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            port: 0,
            advertised_host: Some("127.0.0.1".to_owned()),
            ..ServerConfig::default()
        })
        .await
        .expect("server starts");
        assert!(
            server.endpoint().starts_with("opc.tcp://127.0.0.1:"),
            "{}",
            server.endpoint()
        );
        assert!(!server.endpoint().contains("0.0.0.0"));
        server.shutdown().await.expect("server shuts down");
    }

    #[tokio::test]
    async fn malformed_advertised_hosts_are_rejected_before_bind() {
        for host in [
            "opc.tcp://host",
            "host:4840",
            "host/path",
            "[::1",
            "bad host",
            " host",
        ] {
            let err = OpcUaServer::start(ServerConfig {
                advertised_host: Some(host.to_owned()),
                ..ServerConfig::default()
            })
            .await
            .expect_err("公開ホストには単体のホスト名が必要");
            assert!(
                matches!(err, Error::InvalidAdvertisedHost(_)),
                "{host}: {err}"
            );
        }
    }

    #[tokio::test]
    async fn conflicting_namespace_uri_is_rejected_before_bind() {
        for uri in [
            "",
            " urn:test",
            "http://opcfoundation.org/UA/",
            "urn:banto:opcua:server",
        ] {
            let err =
                OpcUaServer::start(ServerConfig::default().with_namespace_uri(uri).with_port(0))
                    .await
                    .expect_err("名前空間 URI の衝突を拒否する");
            assert!(matches!(err, Error::InvalidNamespaceUri(_)), "{uri}: {err}");
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn initial_value_cannot_overwrite_a_recreated_node() {
        let server = OpcUaServer::start(ServerConfig::default().with_port(0))
            .await
            .expect("サーバーを起動する");
        let (inserted_tx, inserted_rx) = mpsc::channel();
        let (resume_tx, resume_rx) = mpsc::channel();
        let (removing_tx, removing_rx) = mpsc::channel();

        std::thread::scope(|scope| {
            let first_server = &server;
            let first = scope.spawn(move || {
                first_server.fixed_tags.add_tag_with(
                    &first_server.handle,
                    "PLC2",
                    "Cell1",
                    "Speed",
                    Variant::Double(10.0),
                    || {
                        inserted_tx.send(()).expect("挿入を知らせる");
                        resume_rx
                            .recv_timeout(Duration::from_secs(5))
                            .expect("初回の追加を再開する");
                    },
                )
            });
            inserted_rx
                .recv_timeout(Duration::from_secs(5))
                .expect("初回の挿入を待つ");
            assert!(
                server
                    .fixed_tags
                    .manager
                    .address_space()
                    .try_write()
                    .is_none(),
                "初回の追加が write ロックを保持している"
            );
            let second_server = &server;
            let second = scope.spawn(move || {
                removing_tx.send(()).expect("削除開始を知らせる");
                second_server
                    .remove_tag("PLC2", "Cell1", "Speed")
                    .expect("旧タグを削除する");
                second_server
                    .add_tag("PLC2", "Cell1", "Speed", MappedValue::Double(20.0))
                    .expect("同じ NodeId を再登録する");
            });
            removing_rx
                .recv_timeout(Duration::from_secs(5))
                .expect("削除開始を待つ");
            resume_tx.send(()).expect("初回の追加を再開する");
            first
                .join()
                .expect("初回の追加スレッド")
                .expect("初回の追加");
            second.join().expect("再登録スレッド");
        });

        let id = NodeId::new(server.namespace_index(), "PLC2.Cell1.Speed");
        let address_space = server.fixed_tags.manager.address_space();
        {
            let space = address_space.read();
            let Some(NodeType::Variable(variable)) = space.find(&id) else {
                panic!("再登録した変数が存在する");
            };
            let value = variable.value(
                TimestampsToReturn::Both,
                &NumericRange::None,
                &DataEncoding::default(),
                0.0,
            );
            assert_eq!(value.value, Some(Variant::Double(20.0)));
        }
        server.shutdown().await.expect("サーバーを停止する");
    }
}
