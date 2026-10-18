//! `ClientBuilder` で None / Anonymous のセッションを張り、まとめて読み書きする（ADR-0005）。

use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use banto_opcua_core::{NodeAddress, NodeId, ScalarValue, StatusCode};
use futures_util::StreamExt;
use opcua::{
    client::{
        Client, ClientBuilder, IdentityToken, Session, SessionEventLoop, SessionPollResult,
        services, transport::TcpConnector,
    },
    crypto::SecurityPolicy,
    types::{
        AttributeId, DataValue, MessageSecurityMode, NodeId as UaNodeId, ReadValueId,
        StatusCode as UaStatusCode, TimestampsToReturn, UserTokenPolicy, VariableId, Variant,
        WriteRequest, WriteValue,
    },
};
use tokio::{sync::watch, task::JoinHandle};

use crate::{
    ClientConfig, Error, NodeReading, ReconnectPolicy, WriteFailure, WriteOutcome,
    chunk::{ChunkFailure, send_in_chunks},
    convert::{core_status_code, ua_node_id, variant_from_scalar},
    write::{ChunkWriteFailure, classify_response, error_of, outcomes_of},
};

/// クライアントを接続する入口。状態は持たない。
#[derive(Debug, Default)]
pub struct OpcUaClient;

impl OpcUaClient {
    /// `opc.tcp://` URL へ SecurityPolicy `None`、Anonymous で、既定の設定で接続する。
    pub async fn connect(endpoint: &str) -> Result<ConnectedClient, Error> {
        Self::connect_with(ClientConfig::new(endpoint)).await
    }

    /// セッションの確立を待つ時間だけを指定して接続する。
    pub async fn connect_with_session_timeout(
        endpoint: &str,
        session_timeout: Duration,
    ) -> Result<ConnectedClient, Error> {
        Self::connect_with(ClientConfig::new(endpoint).with_connect_timeout(session_timeout)).await
    }

    /// 設定に従って接続し、NamespaceArray と OperationLimits を読む。
    ///
    /// `verify_server_certs(false)` はホスト名検証だけでなく、サーバー証明書の
    /// 検証全体を無効にする。Phase 1 は平文・匿名であり、サンプル証明書では
    /// 検証を通せない。Phase 3 TODO: 証明書検証を戻す。
    ///
    /// セッションの確立を待つ間に時間切れ、呼び出し元のキャンセル、または
    /// async-opcua が接続を諦めたときは、イベントループを中止して `Err` を返す。
    /// 接続後の NamespaceArray と OperationLimits の Read は、サービス自体が
    /// 失敗したときだけ `Err` にする（ノード単位の失敗は既定値を使う）。
    pub async fn connect_with(config: ClientConfig) -> Result<ConnectedClient, Error> {
        config.validate().map_err(Error::Build)?;
        let pki = tempfile::tempdir().map_err(Error::Pki)?;
        let builder = ClientBuilder::new()
            .application_name("banto-opcua-client")
            .application_uri("urn:banto:opcua:client")
            .product_uri("urn:banto:opcua")
            .create_sample_keypair(true)
            .pki_dir(pki.path())
            .trust_server_certs(true)
            // 証明書検証全体を無効化する。ホスト名だけのスイッチではない。
            // Phase 3 TODO: 検証を戻す。
            .verify_server_certs(false)
            .request_timeout(config.request_timeout)
            .keep_alive_interval(config.keep_alive_interval)
            .max_failed_keep_alive_count(config.max_failed_keep_alive_count.get());
        let builder = match config.reconnect {
            ReconnectPolicy::Disabled => builder.session_retry_limit(0),
            ReconnectPolicy::Automatic {
                initial_delay,
                max_delay,
                max_retries,
            } => builder
                .session_retry_limit(
                    max_retries.map_or(-1, |retries| i32::try_from(retries).unwrap_or(i32::MAX)),
                )
                .session_retry_initial(initial_delay)
                .session_retry_max(max_delay),
        };
        let mut client = builder
            .client()
            .map_err(|reasons| Error::Build(reasons.join("; ")))?;

        let (session, event_loop) = client
            .connect_to_matching_endpoint(
                (
                    config.endpoint_url.as_str(),
                    SecurityPolicy::None.to_str(),
                    MessageSecurityMode::None,
                    UserTokenPolicy::anonymous(),
                ),
                IdentityToken::Anonymous,
            )
            .await
            .map_err(|err| Error::Connect(err.to_string()))?;
        if config.reconnect == ReconnectPolicy::Disabled {
            // session_retry_limit(0) だけでは、切断後に 1 回張り直しを試す。
            // イベントループを動かす前に止めれば、初回の接続とも競合しない。
            session.disable_reconnects();
        }

        let (link_tx, mut link) = watch::channel(Link::Connecting);
        // 時間切れや未接続で抜けると、このガードの Drop がタスクを abort する。
        // 成功したときだけ disarm して ConnectedClient へ所有権を渡す。
        let task = AbortOnDrop::new(tokio::spawn(drive(event_loop, link_tx)));
        let established = tokio::time::timeout(
            config.connect_timeout,
            link.wait_for(|state| !matches!(state, Link::Connecting)),
        )
        .await
        .map_err(|_| Error::Connect("接続待ちが時間切れになりました".to_owned()))?
        .map(|state| *state);
        let generation = match established {
            Ok(Link::Connected(generation)) => generation,
            Ok(Link::Closed(Some(status))) => {
                return Err(Error::Connect(format!(
                    "セッションが確立しませんでした: {status}"
                )));
            }
            _ => return Err(Error::Connect("セッションが確立しませんでした".to_owned())),
        };

        let info = match ServerInfo::read(&session, generation, &config).await {
            Ok(info) => info,
            Err(reason) => {
                // セッションを閉じてから返す。abort だけだとサーバーに
                // セッションの時間切れまで残る。
                let _ = tokio::time::timeout(config.request_timeout, session.disconnect()).await;
                return Err(Error::Connect(reason));
            }
        };
        Ok(ConnectedClient {
            session,
            link,
            info: tokio::sync::Mutex::new(Arc::new(info)),
            config,
            task: Some(task.disarm()),
            _client: client,
            _pki: pki,
        })
    }
}

/// イベントループから見た接続の状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Link {
    /// 最初の接続を待っている。
    Connecting,
    /// 接続している。数は接続し直すたびに増える（最初の接続が 1）。
    Connected(u64),
    /// 切れて、自動再接続で張り直している。
    Reconnecting,
    /// イベントループが終わった。以降は接続しない。諦めた理由があれば持つ。
    Closed(Option<StatusCode>),
}

/// イベントループを回し、接続と切断を `link` へ知らせる。
async fn drive(event_loop: SessionEventLoop<TcpConnector>, link: watch::Sender<Link>) {
    let events = event_loop.enter();
    let mut events = std::pin::pin!(events);
    let mut generation = 0;
    loop {
        match events.next().await {
            Some(Ok(SessionPollResult::Reconnected(_))) => {
                generation += 1;
                link.send_replace(Link::Connected(generation));
            }
            Some(Ok(SessionPollResult::ConnectionLost(_))) => {
                link.send_replace(Link::Reconnecting);
            }
            Some(Ok(_)) => {}
            Some(Err(status)) => {
                link.send_replace(Link::Closed(Some(core_status_code(status))));
                return;
            }
            None => {
                link.send_replace(Link::Closed(None));
                return;
            }
        }
    }
}

/// 接続ごとに読む、サーバーの名前空間と上限。
#[derive(Debug)]
struct ServerInfo {
    /// どの接続で読んだか（[`Link::Connected`] の数）。
    generation: u64,
    namespace_array: Vec<String>,
    /// 1 回の Read に入れるノード数。`BadTooManyOperations` で小さくする。
    max_nodes_per_read: AtomicUsize,
    /// 1 回の Write に入れるノード数。`BadTooManyOperations` で小さくする。
    max_nodes_per_write: AtomicUsize,
}

impl ServerInfo {
    async fn read(
        session: &Session,
        generation: u64,
        config: &ClientConfig,
    ) -> Result<Self, String> {
        let nodes = [
            VariableId::Server_NamespaceArray,
            VariableId::Server_ServerCapabilities_OperationLimits_MaxNodesPerRead,
            VariableId::Server_ServerCapabilities_OperationLimits_MaxNodesPerWrite,
        ]
        .map(|id| ReadValueId::from(UaNodeId::from(id)));
        // 上限を知る前の Read なので、MaxNodesPerRead が 1 のサーバーにも分けて送る。
        let mut chunk_size = nodes.len();
        let values = send_in_chunks(
            &nodes,
            &mut chunk_size,
            is_too_many_operations,
            |chunk| async move { session.read(&chunk, TimestampsToReturn::Neither, 0.0).await },
        )
        .await
        .map_err(|err| {
            format!(
                "サーバーの情報を読めませんでした: {}",
                describe_failure(err.failure)
            )
        })?;
        let [namespace_array, max_nodes_per_read, max_nodes_per_write] =
            <[DataValue; 3]>::try_from(values)
                .map_err(|_| "サーバーの情報の応答の数が要求と合いません".to_owned())?;
        Ok(Self {
            generation,
            namespace_array: namespace_array_from(namespace_array),
            max_nodes_per_read: AtomicUsize::new(
                limit_from(max_nodes_per_read).unwrap_or(config.default_max_nodes_per_read.get()),
            ),
            max_nodes_per_write: AtomicUsize::new(
                limit_from(max_nodes_per_write).unwrap_or(config.default_max_nodes_per_write.get()),
            ),
        })
    }
}

/// サービス全体の拒否のうち、塊を小さくすれば通るもの。
fn is_too_many_operations(error: &opcua::types::Error) -> bool {
    error.status() == UaStatusCode::BadTooManyOperations
}

fn describe_failure(failure: ChunkFailure<opcua::types::Error>) -> String {
    match failure {
        ChunkFailure::Request { error, .. } => {
            format!("{}: {error}", core_status_code(error.status()))
        }
        ChunkFailure::CountMismatch { expected, actual } => {
            format!("応答の結果の数が要求と合いません（要求 {expected} 件、結果 {actual} 件）")
        }
    }
}

/// [`ConnectedClient::read_batch`] の塊が失敗した理由。
#[derive(Debug)]
enum ReadFailure {
    /// Read サービスが失敗した。
    Service(opcua::types::Error),
    /// 塊を送る前か応答を受け取った後に、接続が張り直されていた。
    Reconnected,
}

/// 値が String の配列でなければ空にする。`nsu=` のノードはノード単位の `BadNodeIdUnknown` になる。
fn namespace_array_from(value: DataValue) -> Vec<String> {
    if value.status.is_some_and(|status| status.is_bad()) {
        return Vec::new();
    }
    match value.value {
        Some(Variant::Array(array)) => array
            .values
            .into_iter()
            .map(|entry| match entry {
                Variant::String(uri) => uri.value().clone().unwrap_or_default(),
                _ => String::new(),
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// 0（制限なし）、値が無い、Bad のときは `None`（設定の既定値を使う）。
fn limit_from(value: DataValue) -> Option<usize> {
    if value.status.is_some_and(|status| status.is_bad()) {
        return None;
    }
    match value.value {
        Some(Variant::UInt32(limit)) if limit > 0 => usize::try_from(limit).ok(),
        _ => None,
    }
}

/// `spawn` したタスクを、成功パスで所有権を渡すまで abort する。
#[must_use = "成功時に disarm しないとタスクが abort される"]
struct AbortOnDrop<T> {
    handle: Option<JoinHandle<T>>,
}

impl<T> AbortOnDrop<T> {
    /// 直後の失敗パスでもタスクが残らないよう、spawn の戻り値を包む。
    fn new(handle: JoinHandle<T>) -> Self {
        Self {
            handle: Some(handle),
        }
    }

    /// 接続成功時にタスクの所有権を取り出す。以降の Drop では abort しない。
    fn disarm(mut self) -> JoinHandle<T> {
        self.handle.take().expect("接続タスクは成功パスでのみ外す")
    }
}

impl<T> Drop for AbortOnDrop<T> {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            handle.abort();
        }
    }
}

/// 接続済みのセッション。
///
/// [`ReconnectPolicy::Disabled`]（既定）では、接続が切れると以降の呼び出しは
/// すべて [`Error::Disconnected`] を返す。張り直すのは呼び出し側で、新しく
/// 接続する。[`ReconnectPolicy::Automatic`] では、張り直している間の呼び出しが
/// [`Error::Disconnected`] を返し、張り直した後は NamespaceArray と OperationLimits を
/// 読み直してから要求を送る。
#[must_use = "切断するまで ConnectedClient を保持する"]
pub struct ConnectedClient {
    session: Arc<Session>,
    link: watch::Receiver<Link>,
    info: tokio::sync::Mutex<Arc<ServerInfo>>,
    config: ClientConfig,
    task: Option<JoinHandle<()>>,
    _client: Client,
    _pki: tempfile::TempDir,
}

impl ConnectedClient {
    /// まだ `generation` の接続のままか。
    fn on_connection(&self, generation: u64) -> bool {
        *self.link.borrow() == Link::Connected(generation)
    }

    /// 接続していれば、その接続で読んだサーバーの情報を返す。張り直した後なら読み直す。
    async fn server_info(&self) -> Result<Arc<ServerInfo>, Error> {
        let Link::Connected(generation) = *self.link.borrow() else {
            return Err(Error::Disconnected);
        };
        let mut info = self.info.lock().await;
        if info.generation != generation {
            *info = Arc::new(
                ServerInfo::read(&self.session, generation, &self.config)
                    .await
                    .map_err(Error::Read)?,
            );
        }
        Ok(Arc::clone(&info))
    }

    /// ノードの並びの Value 属性をまとめて読み、ノードごとの結果を要求の順に返す。
    ///
    /// - `TimestampsToReturn` は `Source`、`maxAge` は 0（機器から読み直す）。
    /// - サーバーの `MaxNodesPerRead` を超える要求は分けて送る。`BadTooManyOperations`
    ///   が返ったら塊を半分にして送り直し、1 ノードでも拒否されたら `Err`。
    /// - 空の並びは送らずに空の結果を返す。
    /// - `nsu=` の URI が NamespaceArray に無いノードは、送らずに `BadNodeIdUnknown`。
    /// - 切断、時間切れ、サービス全体のエラー、応答の数の不一致は `Err`（接続ごとの
    ///   失敗）。ノード単位の失敗は結果の [`NodeReading::status`] にする。
    /// - 自動再接続のとき、塊を送る前か応答を受け取った後に接続が張り直されていたら、
    ///   [`Error::Disconnected`] にする。張り直した先では名前空間の番号が変わりうるので、
    ///   古い番号で解決した要求の結果は返さない。次の呼び出しで解決し直す。
    pub async fn read_batch(&self, nodes: &[NodeAddress]) -> Result<Vec<NodeReading>, Error> {
        if nodes.is_empty() {
            return Ok(Vec::new());
        }
        let info = self.server_info().await?;
        let mut readings: Vec<Option<NodeReading>> = vec![None; nodes.len()];
        let mut positions = Vec::with_capacity(nodes.len());
        let mut requests = Vec::with_capacity(nodes.len());
        for (position, address) in nodes.iter().enumerate() {
            match address.resolve(&info.namespace_array) {
                Some(node_id) => {
                    positions.push(position);
                    requests.push(ReadValueId {
                        node_id: ua_node_id(&node_id),
                        attribute_id: AttributeId::Value as u32,
                        ..Default::default()
                    });
                }
                None => {
                    readings[position] =
                        Some(NodeReading::without_value(StatusCode::BAD_NODE_ID_UNKNOWN));
                }
            }
        }

        let generation = info.generation;
        let mut chunk_size = info.max_nodes_per_read.load(Ordering::Relaxed);
        let result = send_in_chunks(
            &requests,
            &mut chunk_size,
            |failure| matches!(failure, ReadFailure::Service(error) if is_too_many_operations(error)),
            |chunk| async move {
                if !self.on_connection(generation) {
                    return Err(ReadFailure::Reconnected);
                }
                let values = self
                    .session
                    .read(&chunk, TimestampsToReturn::Source, 0.0)
                    .await
                    .map_err(ReadFailure::Service)?;
                if !self.on_connection(generation) {
                    return Err(ReadFailure::Reconnected);
                }
                Ok(values)
            },
        )
        .await;
        info.max_nodes_per_read.store(chunk_size, Ordering::Relaxed);
        let values = result.map_err(|err| match err.failure {
            ChunkFailure::Request {
                error: ReadFailure::Reconnected,
                ..
            } => Error::Disconnected,
            ChunkFailure::Request {
                error: ReadFailure::Service(error),
                chunk_len,
            } => Error::Read(describe_failure(ChunkFailure::Request { error, chunk_len })),
            ChunkFailure::CountMismatch { expected, actual } => {
                Error::Read(describe_failure(ChunkFailure::CountMismatch {
                    expected,
                    actual,
                }))
            }
        })?;
        // 要求を 1 つも送らなかった（全ノードの URI が解決できなかった）ときも、
        // 解決に使った NamespaceArray がまだその接続のものかを確かめる。
        if !self.on_connection(generation) {
            return Err(Error::Disconnected);
        }
        for (position, value) in positions.into_iter().zip(values) {
            readings[position] = Some(NodeReading::from_data_value(value));
        }
        Ok(readings.into_iter().flatten().collect())
    }

    /// ノードと値の並びを Value 属性へまとめて書き、ノードごとの StatusCode を要求の順に返す。
    ///
    /// - DataValue には値だけを入れる（StatusCode も時刻も付けない）。IndexRange は付けない。
    ///   型は変換しない。ノードの型と違えばサーバーが `BadTypeMismatch` を返す。
    /// - サーバーの `MaxNodesPerWrite` を超える要求は分けて送る。`BadTooManyOperations` が
    ///   返ったら、拒否された塊とそれ以降を半分にして送り直す（拒否された塊は適用されて
    ///   いない）。1 ノードの塊でも拒否されたら失敗にする。
    /// - 空の並びは送らずに空の結果を返す。
    /// - `nsu=` の URI が NamespaceArray に無いノードは、送らずに `BadNodeIdUnknown`。
    ///
    /// 接続ごとの失敗（切断、時間切れ、要求ごとの拒否、応答の不一致）では
    /// [`WriteFailure`] を返す。先に送った塊は機器に届いているので、ノードごとの結末
    /// （[`WriteOutcome`]）を要求の順に持たせる。自動再接続のとき、塊を送る前に
    /// 張り直していたらその塊から先は送らず、応答の後に張り直していたら（古い名前空間
    /// 番号で送った可能性がある）その塊を結果不明にする。
    pub async fn write_batch(
        &self,
        writes: &[(NodeAddress, ScalarValue)],
    ) -> Result<Vec<StatusCode>, WriteFailure> {
        if writes.is_empty() {
            return Ok(Vec::new());
        }
        let info = self
            .server_info()
            .await
            .map_err(|error| WriteFailure::nothing_sent(error, writes.len()))?;
        let mut statuses: Vec<Option<StatusCode>> = vec![None; writes.len()];
        let mut positions = Vec::with_capacity(writes.len());
        let mut requests = Vec::with_capacity(writes.len());
        for (position, (address, value)) in writes.iter().enumerate() {
            match address.resolve(&info.namespace_array) {
                Some(node_id) => {
                    positions.push(position);
                    requests.push(WriteValue {
                        node_id: ua_node_id(&node_id),
                        attribute_id: AttributeId::Value as u32,
                        index_range: Default::default(),
                        value: DataValue {
                            value: Some(variant_from_scalar(value)),
                            ..Default::default()
                        },
                    });
                }
                None => statuses[position] = Some(StatusCode::BAD_NODE_ID_UNKNOWN),
            }
        }

        let generation = info.generation;
        let mut chunk_size = info.max_nodes_per_write.load(Ordering::Relaxed);
        let result = send_in_chunks(
            &requests,
            &mut chunk_size,
            ChunkWriteFailure::is_too_many_operations,
            |chunk| async move {
                if !self.on_connection(generation) {
                    return Err(ChunkWriteFailure::NotSent);
                }
                let chunk_len = chunk.len();
                let request = WriteRequest {
                    request_header: services::Write::new(&self.session).header().clone(),
                    nodes_to_write: Some(chunk),
                };
                let response = self
                    .session
                    .channel()
                    .send(request, self.config.request_timeout)
                    .await;
                let results = classify_response(response, chunk_len)?;
                if !self.on_connection(generation) {
                    return Err(ChunkWriteFailure::Unknown(
                        StatusCode::BAD_CONNECTION_CLOSED,
                    ));
                }
                Ok(results)
            },
        )
        .await;
        info.max_nodes_per_write
            .store(chunk_size, Ordering::Relaxed);

        // 送ったノードの結末を、要求の順の位置へ戻す。送らずに決めたノードは
        // Completed（BadNodeIdUnknown）。
        let place = |sent: Vec<WriteOutcome>| -> Vec<WriteOutcome> {
            let mut outcomes: Vec<_> = statuses
                .iter()
                .map(|status| status.map_or(WriteOutcome::NotSent, WriteOutcome::Completed))
                .collect();
            for (position, outcome) in positions.iter().zip(sent) {
                outcomes[*position] = outcome;
            }
            outcomes
        };
        match result {
            Ok(results) => {
                // 要求を 1 つも送らなかったときも、解決に使った NamespaceArray が
                // まだその接続のものかを確かめる。
                if !self.on_connection(generation) {
                    let sent = results
                        .into_iter()
                        .map(|status| WriteOutcome::Completed(core_status_code(status)))
                        .collect();
                    let mut outcomes = place(sent);
                    for (outcome, status) in outcomes.iter_mut().zip(&statuses) {
                        if status.is_some() {
                            *outcome = WriteOutcome::NotSent;
                        }
                    }
                    return Err(WriteFailure {
                        error: Error::Disconnected,
                        outcomes,
                    });
                }
                for (position, status) in positions.into_iter().zip(results) {
                    statuses[position] = Some(core_status_code(status));
                }
                Ok(statuses.into_iter().flatten().collect())
            }
            Err(error) => {
                let (sent, failure) = outcomes_of(error, requests.len());
                Err(WriteFailure {
                    error: error_of(failure),
                    outcomes: place(sent),
                })
            }
        }
    }

    /// `ServerStatus/State` を読む。OPC UA では Running が 0。
    ///
    /// 接続が切れている（張り直している）ときは [`Error::Disconnected`]。
    pub async fn read_server_state(&self) -> Result<i32, Error> {
        let value = self
            .read_value(VariableId::Server_ServerStatus_State.into())
            .await?;
        match value {
            Variant::Int32(state) => Ok(state),
            other => Err(Error::Read(format!(
                "State の型が Int32 ではありません: {other:?}"
            ))),
        }
    }

    /// 指定した NodeId の UInt32 値を読む。外部サーバーのタグ Read に使う。
    /// 値が無い、StatusCode が Bad、または型が UInt32 以外の場合はエラーにする。
    /// 接続が切れている（張り直している）ときは [`Error::Disconnected`]。
    pub async fn read_u32(&self, node_id: &NodeId) -> Result<u32, Error> {
        let value = self.read_value(ua_node_id(node_id)).await?;
        match value {
            Variant::UInt32(number) => Ok(number),
            other => Err(Error::Read(format!(
                "{node_id} の型が UInt32 ではありません: {other:?}"
            ))),
        }
    }

    /// 1 ノードを読む。接続が切れている（張り直している）ときは [`Error::Disconnected`]。
    async fn read_value(&self, node_id: UaNodeId) -> Result<Variant, Error> {
        let connected = || matches!(*self.link.borrow(), Link::Connected(_));
        if !connected() {
            return Err(Error::Disconnected);
        }
        let values = self
            .session
            .read(
                &[ReadValueId {
                    node_id,
                    attribute_id: AttributeId::Value as u32,
                    ..Default::default()
                }],
                TimestampsToReturn::Both,
                0.0,
            )
            .await
            .map_err(|err| {
                if connected() {
                    Error::Read(err.to_string())
                } else {
                    Error::Disconnected
                }
            })?;
        let value = values
            .into_iter()
            .next()
            .ok_or_else(|| Error::Read("応答が空でした".to_owned()))?;
        if let Some(status) = value.status
            && status.is_bad()
        {
            return Err(Error::Read(format!("StatusCode が Bad でした: {status}")));
        }
        value
            .value
            .ok_or_else(|| Error::Read("値がありません".to_owned()))
    }

    /// セッションを閉じてイベントループの終了を待つ。
    ///
    /// 接続がすでに切れていれば、閉じる要求は送らずにイベントループを止める。
    /// 閉じる要求が `request_timeout` の 2 倍で終わらなければ（自動再接続で
    /// 切断と張り直しが重なったときなど）、イベントループを止めて `Err` を返す。
    pub async fn disconnect(mut self) -> Result<(), Error> {
        let connected = matches!(*self.link.borrow(), Link::Connected(_));
        let result = if connected {
            let limit = self.config.request_timeout.saturating_mul(2);
            match tokio::time::timeout(limit, self.session.disconnect()).await {
                Ok(result) => result.map_err(|err| Error::Connect(err.to_string())),
                Err(_) => Err(Error::Connect("切断が時間切れになりました".to_owned())),
            }
        } else {
            Ok(())
        };
        if let Some(task) = self.task.take() {
            if !connected || result.is_err() {
                task.abort();
            }
            let _ = task.await;
        }
        result
    }
}

impl std::fmt::Debug for ConnectedClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConnectedClient").finish_non_exhaustive()
    }
}

impl Drop for ConnectedClient {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    use opcua::types::{Array, UAString, VariantScalarTypeId};

    use super::*;

    /// タスクが drop されたことを記録する。
    struct FlagOnDrop(Arc<AtomicBool>);

    impl Drop for FlagOnDrop {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    fn pending_task(dropped: &Arc<AtomicBool>) -> JoinHandle<()> {
        let flag = FlagOnDrop(Arc::clone(dropped));
        tokio::spawn(async move {
            let _flag = flag;
            std::future::pending::<()>().await;
        })
    }

    #[tokio::test]
    async fn abort_on_drop_stops_the_spawned_task() {
        let dropped = Arc::new(AtomicBool::new(false));
        let guard = AbortOnDrop::new(pending_task(&dropped));
        drop(guard);
        wait_for_task_drop(&dropped).await;
    }

    #[tokio::test]
    async fn disarm_transfers_the_task_without_aborting_it() {
        let dropped = Arc::new(AtomicBool::new(false));
        let handle = AbortOnDrop::new(pending_task(&dropped)).disarm();
        tokio::task::yield_now().await;
        assert!(!dropped.load(Ordering::SeqCst));
        assert!(!handle.is_finished());
        handle.abort();
        assert!(handle.await.expect_err("abort されたタスク").is_cancelled());
        wait_for_task_drop(&dropped).await;
    }

    async fn wait_for_task_drop(dropped: &Arc<AtomicBool>) {
        tokio::time::timeout(Duration::from_secs(1), async {
            while !dropped.load(Ordering::SeqCst) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("spawn したタスクが中止されず残っている");
    }

    fn data_value(value: Variant, status: Option<UaStatusCode>) -> DataValue {
        DataValue {
            value: Some(value),
            status,
            ..Default::default()
        }
    }

    #[test]
    fn advertised_limit_is_used_unless_zero_missing_or_bad() {
        assert_eq!(limit_from(data_value(Variant::UInt32(7), None)), Some(7));
        assert_eq!(limit_from(data_value(Variant::UInt32(0), None)), None);
        assert_eq!(
            limit_from(data_value(
                Variant::UInt32(7),
                Some(UaStatusCode::BadNodeIdUnknown)
            )),
            None
        );
        assert_eq!(limit_from(DataValue::default()), None);
        assert_eq!(limit_from(data_value(Variant::Int32(7), None)), None);
    }

    #[test]
    fn namespace_array_is_empty_unless_a_string_array() {
        let array = Variant::Array(Box::new(
            Array::new(
                VariantScalarTypeId::String,
                vec![
                    Variant::String(UAString::from("http://opcfoundation.org/UA/")),
                    Variant::String(UAString::from("urn:a")),
                ],
            )
            .unwrap(),
        ));
        assert_eq!(
            namespace_array_from(data_value(array.clone(), None)),
            vec!["http://opcfoundation.org/UA/", "urn:a"]
        );
        assert!(
            namespace_array_from(data_value(array, Some(UaStatusCode::BadNodeIdUnknown)))
                .is_empty()
        );
        assert!(namespace_array_from(data_value(Variant::Int32(1), None)).is_empty());
    }
}
