//! async-opcua の `ServerBuilder` を、ループバック既定の待受で包む。
//!
//! Phase 1 の固定タグと、追加・削除できるタグを Objects 以下に公開する。

use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
};

use opcua::{
    server::{ANONYMOUS_USER_TOKEN_ID, ServerBuilder, ServerEndpoint, ServerHandle},
    types::{DataValue, DateTime, Variant},
};
use tokio::{net::TcpListener, task::JoinHandle};

use banto_opcua_core::{DataType, MappedValue, Quality, map_current_value};

use crate::{
    Error, ServerConfig, TagReading,
    config::{
        host_is_loopback, host_is_unspecified, normalize_endpoint_path, valid_advertised_host,
    },
    fixed_tags::{self, FixedTags},
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
    /// `pki_dir` を作れないとき、または Unix で残す秘密鍵の権限を絞れないときは
    /// [`Error::Pki`] を返す。
    pub async fn start(mut config: ServerConfig) -> Result<RunningServer, Error> {
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
        }

        let namespace_uri = config.namespace_uri.clone();
        let mut builder = ServerBuilder::new()
            .application_name(config.application_name)
            .application_uri(config.application_uri)
            .product_uri(config.product_uri)
            .create_sample_keypair(true)
            .certificate_path("own/cert.der")
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
            .with_node_manager(fixed_tags::node_manager(namespace_uri.clone()));
        // async-opcua の既定値 100 ms は 0 要求も補正する。厳密な採取時刻が
        // 必要なクライアントには、実際に 0 ms の監視項目を作れるようにする。
        builder.limits_mut().subscriptions.min_sampling_interval_ms = 0.0;
        let (server, handle) = builder.build().map_err(Error::Build)?;
        pki.restrict_private_key()?;
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

const PRIVATE_KEY_PATH: &str = "private/private.pem";

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

    /// 残す秘密鍵を所有者だけが読めるようにする。async-opcua は umask の
    /// 権限で書くため、作成直後と既存の鍵の両方をここで絞る。一時ディレクトリは
    /// tempfile が所有者だけに作り、停止で消えるので触らない。
    #[cfg(unix)]
    fn restrict_private_key(&self) -> Result<(), Error> {
        use std::os::unix::fs::PermissionsExt;

        let Self::Persistent(dir) = self else {
            return Ok(());
        };
        let key = dir.join(PRIVATE_KEY_PATH);
        if let Some(dir) = key.parent() {
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
                .map_err(Error::Pki)?;
        }
        std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o600)).map_err(Error::Pki)
    }

    #[cfg(not(unix))]
    fn restrict_private_key(&self) -> Result<(), Error> {
        Ok(())
    }
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

    /// `{connection}.{group}.{tag}` の読み取り専用タグを追加する。
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

    /// 実データの型・品質・時刻を保持したまま、読み取り専用タグを登録する。
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

    /// タグを削除し、既存の購読へ `BadNodeIdUnknown` を通知する。
    /// MonitoredItem は保持し、同じ NodeId の再登録で通知が再開する。
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
        let reading = map_current_value(DataType::Double, speed, quality);
        let now = DateTime::now();
        let value = DataValue {
            // upstream の Variable::value は Value フィールド自体が None だと
            // StatusCode も落とす。Null Variant で値なしを表して Bad を保つ。
            // 回避策の見直しは #17 で追跡する。
            value: Some(match reading.value {
                Some(MappedValue::Boolean(bit)) => Variant::Boolean(bit),
                Some(MappedValue::Double(number)) => Variant::Double(number),
                None => Variant::Empty,
            }),
            status: Some(reading.status.code().into()),
            source_timestamp: Some(now),
            source_picoseconds: None,
            server_timestamp: Some(now),
            server_picoseconds: None,
        };
        self.fixed_tags.update(&self.handle, "Speed", value)
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
