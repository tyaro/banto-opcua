//! async-opcua の `ServerBuilder` を、ループバック既定の待受で包む。
//!
//! Phase 1 の固定タグと、追加・削除できるタグを Objects 以下に公開する。
//! エンドポイント、利用者の認証、証明書の信頼の一覧は ADR-0009。

use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
    time::SystemTime,
};

use opcua::{
    server::{ServerBuilder, ServerEndpoint, ServerHandle},
    types::DataValue,
};
use tokio::{net::TcpListener, task::JoinHandle};

use banto_opcua_core::{
    CertificateInfo, DataType, MAX_STRING_BYTES, MappedValue, Quality, SampleTimestamps,
    TagMetadata,
};
use banto_opcua_pki::{
    ApplicationIdentity, CERTIFICATE_PATH, PRIVATE_KEY_PATH, TrustList, prepare_own_certificate,
};

use crate::{
    Error, ServerConfig, ServerHandlers, TagReading, WriteHandler,
    config::{
        host_is_loopback, host_is_unspecified, normalize_endpoint_path, valid_advertised_host,
    },
    fixed_tags::{self, FixedTags},
    reading,
    security::{Gatekeeper, SecurityEvent, SecurityLog, ua_mode, ua_policy},
    tag_manager::WritePath,
};

/// 直近の [`SecurityEvent`] を残す数。
const SECURITY_LOG_CAPACITY: usize = 256;

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
    /// 公開するときは警告ログを出す。エンドポイントの組み合わせが成り立たないときは
    /// [`Error::InvalidSecurity`]（ADR-0009）。
    ///
    /// `pki_dir` を作れないとき、証明書と秘密鍵の組を作れなかったとき、または
    /// Unix で残す秘密鍵の権限を絞れないときは [`Error::Pki`] を返す。
    ///
    /// 書き込みハンドラーを渡さないので、すべてのタグは読み取り専用になる。
    /// 利用者の照合も渡さないので、受け付けるのは Anonymous だけ。
    pub async fn start(config: ServerConfig) -> Result<RunningServer, Error> {
        Self::start_with_handlers(config, ServerHandlers::default()).await
    }

    /// 書き込みハンドラーを渡して起動する（ADR-0004）。
    ///
    /// [`Self::start_with_handlers`] に書き込みハンドラーだけを渡すのと同じ。
    pub async fn start_with_write_handler(
        config: ServerConfig,
        handler: Arc<dyn WriteHandler>,
    ) -> Result<RunningServer, Error> {
        Self::start_with_handlers(
            config,
            ServerHandlers::default().with_write_handler(handler),
        )
        .await
    }

    /// 書き込みハンドラーと利用者の照合を渡して起動する（ADR-0004、ADR-0009）。
    ///
    /// 書き込みハンドラーがあれば、[`RunningServer::set_tag_writable`] で書き込み可に
    /// したタグへの Write を、サーバーの検査を通したうえで渡す。利用者の照合があれば、
    /// 暗号化したエンドポイントで UserName を受け付ける。Anonymous で書ける設定のまま
    /// ループバック以外へ公開するときは警告ログを出す。ほかは [`Self::start`] と同じ。
    pub async fn start_with_handlers(
        mut config: ServerConfig,
        handlers: ServerHandlers,
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
        let endpoints = config.endpoint_security().map_err(Error::InvalidSecurity)?;
        if !config.allow_anonymous && handlers.user_authenticator.is_none() {
            return Err(Error::InvalidSecurity(
                "Anonymous を許さないなら、利用者の照合を渡してください".to_owned(),
            ));
        }

        let pki = Pki::prepare(config.pki_dir.take())?;
        let listener = TcpListener::bind((config.bind, config.port))
            .await
            .map_err(Error::Bind)?;
        let local_addr = listener.local_addr().map_err(Error::Bind)?;
        let path = normalize_endpoint_path(&config.endpoint_path);
        let endpoint_host = config.endpoint_host(local_addr);
        let endpoint = config.endpoint_url(local_addr);
        let mut identity =
            ApplicationIdentity::new(&config.application_name, &config.application_uri)
                .with_host(&endpoint_host);
        if !config.bind.is_unspecified() {
            identity = identity.with_host(&config.bind.to_string());
        }
        let certificate = prepare_own_certificate(pki.path(), &identity).map_err(Error::Pki)?;
        let certificate_der =
            std::fs::read(pki.path().join(CERTIFICATE_PATH)).map_err(Error::Pki)?;

        let public = !config.bind.is_loopback() || !host_is_loopback(&endpoint_host);
        if public {
            tracing::warn!(
                bind = %config.bind,
                advertised_host = %endpoint_host,
                "OPC UA サーバーをループバック以外へ公開します"
            );
            if config.none_endpoint {
                tracing::warn!(
                    "SecurityPolicy None のエンドポイントを開いています。通信は平文で、Anonymous で接続できます"
                );
            }
            if handlers.write_handler.is_some() && config.allow_anonymous {
                tracing::warn!(
                    bind = %config.bind,
                    advertised_host = %endpoint_host,
                    "OPC UA の書き込みを受け付けます。信頼された証明書を持つクライアント（None のエンドポイントがあれば誰でも）が Anonymous で書けます"
                );
            }
        }
        if config.trust_all_client_certificates {
            tracing::warn!(
                "知らないクライアントの証明書も信頼します（試験用）。証明書を持つ誰でも接続できます"
            );
        }

        let log = Arc::new(SecurityLog::new(SECURITY_LOG_CAPACITY));
        let gatekeeper = Gatekeeper {
            allow_anonymous: config.allow_anonymous,
            users: handlers.user_authenticator,
            log: log.clone(),
        };
        let namespace_uri = config.namespace_uri.clone();
        let mut builder = ServerBuilder::new()
            .application_name(config.application_name)
            .application_uri(config.application_uri)
            .product_uri(config.product_uri)
            .create_sample_keypair(false)
            .certificate_path(CERTIFICATE_PATH)
            .private_key_path(PRIVATE_KEY_PATH)
            .pki_dir(pki.path())
            .host(endpoint_host)
            .port(local_addr.port())
            .trust_client_certs(config.trust_all_client_certificates)
            .with_authenticator(Arc::new(gatekeeper))
            .discovery_urls(vec![endpoint.clone()]);
        for (policy, mode) in endpoints {
            builder = builder.add_endpoint(
                format!("{policy}_{mode}"),
                ServerEndpoint::new(&path, ua_policy(policy), ua_mode(mode), &[]),
            );
        }
        let mut builder = builder.with_node_manager(fixed_tags::node_manager(
            namespace_uri.clone(),
            handlers
                .write_handler
                .map(|handler| WritePath::new(handler, config.max_concurrent_writes)),
        ));
        // async-opcua の既定値 100 ms は 0 要求も補正する。厳密な採取時刻が
        // 必要なクライアントには、実際に 0 ms の監視項目を作れるようにする。
        builder.limits_mut().subscriptions.min_sampling_interval_ms = 0.0;
        builder.limits_mut().operational.max_nodes_per_read = config.max_nodes_per_read.get();
        builder.limits_mut().operational.max_nodes_per_write = config.max_nodes_per_write.get();
        let (server, handle) = builder.build().map_err(Error::Build)?;
        let fixed_tags = FixedTags::register(&handle, &namespace_uri, config.sample_tags)?;
        let pki_dir = pki.path().to_path_buf();

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
            certificate,
            certificate_der,
            trust_list: TrustList::new(pki_dir),
            log,
            task: Some(task),
        })
    }
}

/// 自己署名証明書と信頼の一覧の置き場。一時ディレクトリは停止とともに消え、
/// `pki_dir` は次の起動で同じ鍵と一覧を読み直すために残す。
enum Pki {
    Temporary(tempfile::TempDir),
    Persistent(PathBuf),
}

impl Pki {
    fn prepare(dir: Option<PathBuf>) -> Result<Self, Error> {
        match dir {
            Some(dir) => Ok(Self::Persistent(dir)),
            None => tempfile::tempdir().map(Self::Temporary).map_err(Error::Pki),
        }
    }

    fn path(&self) -> &Path {
        match self {
            Self::Temporary(dir) => dir.path(),
            Self::Persistent(dir) => dir,
        }
    }
}

/// 起動済みのサーバー。
///
/// [`Drop`] はキャンセルと `abort` だけを行い、停止完了は待たない。
/// 鍵と証明書の一時ディレクトリは待受タスクが所有するので、タスクが
/// 終わるまで削除されない。停止完了を待つときは [`Self::shutdown`]。
#[must_use = "シャットダウンするまで RunningServer を保持する"]
pub struct RunningServer {
    local_addr: SocketAddr,
    endpoint: String,
    handle: ServerHandle,
    fixed_tags: FixedTags,
    certificate: CertificateInfo,
    certificate_der: Vec<u8>,
    trust_list: TrustList,
    log: Arc<SecurityLog>,
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

    /// サーバーの証明書の要約。クライアントの管理者が拇印を照らし合わせるのに使う。
    pub fn certificate(&self) -> &CertificateInfo {
        &self.certificate
    }

    /// サーバーの証明書の DER。クライアントの信頼の一覧へ前もって渡すときに使う。
    pub fn certificate_der(&self) -> &[u8] {
        &self.certificate_der
    }

    /// クライアントの証明書の信頼の一覧（ADR-0009）。
    ///
    /// 初めて来たクライアントの証明書は拒否の一覧に置かれ、接続は断られる。管理者が
    /// [`TrustList::trust`] で信頼に移すと、次の接続から受け付ける（再起動は要らない）。
    /// `pki_dir` を設定していなければ、一覧は停止で消える。
    pub fn trust_list(&self) -> &TrustList {
        &self.trust_list
    }

    /// 直近（最大 256 件）の、受け付けた・断ったセッションの記録。古い順。
    ///
    /// 利用者の照合と Anonymous の可否による結果だけを残す。クライアントの証明書による
    /// 拒否は [`Self::trust_list`] の拒否の一覧と、証明書の有効期間で見る（ADR-0009）。
    pub fn security_events(&self) -> Vec<SecurityEvent> {
        self.log.snapshot()
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
        if !value.is_publishable_as(value.data_type()) {
            return Err(Error::TagUpdate(format!(
                "タグの初期値は有限値、String は {MAX_STRING_BYTES} バイト以下にしてください"
            )));
        }
        let value = reading::variant(value);
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
        self.add_tag_reading_with_metadata(connection, group, tag, reading, &TagMetadata::default())
    }

    /// [`Self::add_tag_reading`] に、説明・単位・レンジ・二値の文言を付けて登録する
    /// （ADR-0008）。属性がタグの型に合わなければエラー。
    ///
    /// 属性に応じて型定義を選ぶ。EURange があれば AnalogItemType、単位だけなら
    /// BaseAnalogType、二値の文言があれば TwoStateDiscreteType、ほかは
    /// BaseDataVariableType。プロパティは名前空間 0 の標準の BrowseName で、
    /// HasProperty で辿れる。
    pub fn add_tag_reading_with_metadata(
        &self,
        connection: &str,
        group: &str,
        tag: &str,
        reading: TagReading,
        metadata: &TagMetadata,
    ) -> Result<(), Error> {
        self.fixed_tags
            .add_tag_reading(&self.handle, connection, group, tag, reading, metadata)
    }

    /// 登録済みタグの説明・単位・レンジ・二値の文言を書き換え、購読者へ通知する
    /// （ADR-0008）。
    ///
    /// 同じ NodeId のまま書き換えられるのは、ノードの形（[`TagMetadata::shape`]。
    /// 型定義と持つプロパティ）が同じときだけ。形が変わるなら
    /// [`Error::MetadataShapeChanged`] を返し、何も変えない。そのときは
    /// [`Self::remove_tag`] と [`Self::add_tag_reading_with_metadata`] で登録し直す。
    /// 属性がタグの型に合わないときは [`Error::TagUpdate`]。
    ///
    /// Description 属性とプロパティの Value を購読しているクライアントへは、変更を
    /// 通知する。単位・レンジ・文言の値が変わったら、タグ本体の Value の購読者にも
    /// StatusCode に SemanticsChanged を立てた通知を、監視項目ごとに 1 件送る（値・品質・
    /// 時刻はそのまま）。ビットは保存した値には立てないので、通常の Read には出ない。
    /// サンプルタグには付けられない。
    pub fn update_tag_metadata(
        &self,
        connection: &str,
        group: &str,
        tag: &str,
        metadata: &TagMetadata,
    ) -> Result<(), Error> {
        self.fixed_tags
            .update_tag_metadata(&self.handle, connection, group, tag, metadata)
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
        types::{DataEncoding, NodeId, NumericRange, TimestampsToReturn, Variant},
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
