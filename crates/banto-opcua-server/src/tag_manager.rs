//! タグを置く node manager（ADR-0004）。
//!
//! 読み取りと購読は async-opcua の `InMemoryNodeManagerImpl` の既定実装に任せ、
//! 書き込みだけを自前で扱う。Write はノードの値を書き換えず、検査を通った
//! ものだけを書き込み先へ渡す。書き込み先が無ければすべて `BadNotWritable`。

use std::{future::Future, pin::Pin, sync::Arc};

use async_trait::async_trait;
use opcua::{
    server::{
        address_space::{AccessLevel, AddressSpace, NodeType, user_access_level},
        diagnostics::NamespaceMetadata,
        node_manager::{
            NodeManagerBuilder, RequestContext, ServerContext, WriteNode,
            memory::{InMemoryNodeManager, InMemoryNodeManagerBuilder, InMemoryNodeManagerImpl},
        },
    },
    sync::RwLock,
    types::{AttributeId, DataTypeId, NodeId, NumericRange, StatusCode, Variant},
};

pub(crate) type TagNodeManager = InMemoryNodeManager<TagNodeManagerImpl>;

/// サーバーの検査を通った 1 件の書き込み。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct WriteAttempt {
    pub node_id: NodeId,
    pub value: Variant,
    pub anonymous: bool,
}

pub(crate) type WriteFuture = Pin<Box<dyn Future<Output = StatusCode> + Send + 'static>>;

/// 書き込み先。#25 の本実装で公開のハンドラーに置き換える。
pub(crate) trait WriteSink: Send + Sync + 'static {
    fn write(&self, attempt: WriteAttempt) -> WriteFuture;
}

pub(crate) fn node_manager(
    namespace_uri: String,
    name: &'static str,
    sink: Option<Arc<dyn WriteSink>>,
) -> impl NodeManagerBuilder {
    InMemoryNodeManagerBuilder::new(
        move |context: ServerContext, address_space: &mut AddressSpace| {
            // SimpleNodeManager と同じく、型ツリーとアドレス空間の両方へ名前空間を登録する。
            let namespace_index = context
                .type_tree
                .write()
                .namespaces_mut()
                .add_namespace(&namespace_uri);
            address_space.add_namespace(&namespace_uri, namespace_index);
            TagNodeManagerImpl {
                namespaces: vec![NamespaceMetadata {
                    namespace_uri,
                    namespace_index,
                    ..Default::default()
                }],
                name,
                sink,
            }
        },
    )
}

pub(crate) struct TagNodeManagerImpl {
    namespaces: Vec<NamespaceMetadata>,
    name: &'static str,
    sink: Option<Arc<dyn WriteSink>>,
}

#[async_trait]
impl InMemoryNodeManagerImpl for TagNodeManagerImpl {
    async fn init(&self, _address_space: &mut AddressSpace, _context: ServerContext) {}

    fn name(&self) -> &str {
        self.name
    }

    fn namespaces(&self) -> Vec<NamespaceMetadata> {
        self.namespaces.clone()
    }

    async fn write(
        &self,
        context: &RequestContext,
        address_space: &RwLock<AddressSpace>,
        nodes_to_write: &mut [&mut WriteNode],
    ) -> Result<(), StatusCode> {
        for node in nodes_to_write.iter_mut() {
            let Some(sink) = &self.sink else {
                node.set_status(StatusCode::BadNotWritable);
                continue;
            };
            // 検査はロックの中で終え、書き込み先を待つ間はロックを持たない。
            let checked = check_write(context, &address_space.read(), node);
            let attempt = match checked {
                Ok(attempt) => attempt,
                Err(status) => {
                    node.set_status(status);
                    continue;
                }
            };
            // 要求が捨てられても、始めた 1 件は最後まで走らせる。
            let status = tokio::spawn(sink.write(attempt))
                .await
                .unwrap_or(StatusCode::BadInternalError);
            node.set_status(status);
        }
        Ok(())
    }
}

/// ADR-0004 の「サーバーが返すもの」の検査。
fn check_write(
    context: &RequestContext,
    space: &AddressSpace,
    node: &WriteNode,
) -> Result<WriteAttempt, StatusCode> {
    let request = node.value();
    let found = space
        .find(&request.node_id)
        .ok_or(StatusCode::BadNodeIdUnknown)?;
    let NodeType::Variable(variable) = found else {
        return Err(StatusCode::BadNotWritable);
    };
    if request.attribute_id != AttributeId::Value
        || !variable.access_level().contains(AccessLevel::CURRENT_WRITE)
    {
        return Err(StatusCode::BadNotWritable);
    }
    if !user_access_level(context, found).contains(AccessLevel::CURRENT_WRITE) {
        return Err(StatusCode::BadUserAccessDenied);
    }
    if request.index_range != NumericRange::None {
        return Err(StatusCode::BadIndexRangeNoData);
    }
    if request.value.status.is_some_and(|status| !status.is_good()) {
        return Err(StatusCode::BadWriteNotSupported);
    }
    let value = request
        .value
        .value
        .clone()
        .ok_or(StatusCode::BadTypeMismatch)?;
    let data_type = variable.data_type();
    let matches = match &value {
        Variant::Boolean(_) => data_type == DataTypeId::Boolean,
        Variant::Double(_) => data_type == DataTypeId::Double,
        _ => false,
    };
    if !matches {
        return Err(StatusCode::BadTypeMismatch);
    }
    Ok(WriteAttempt {
        node_id: request.node_id.clone(),
        value,
        anonymous: context.token.is_anonymous(),
    })
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use opcua::{
        client::{ClientBuilder, IdentityToken, Session},
        crypto::SecurityPolicy,
        types::{
            DataValue, DateTime, MessageSecurityMode, ReadValueId, TimestampsToReturn,
            UserTokenPolicy, WriteValue,
        },
    };
    use tokio::sync::mpsc;

    use super::*;
    use crate::{OpcUaServer, RunningServer, ServerConfig, model::MappedValue};

    const WRITE_DELAY: Duration = Duration::from_secs(1);

    /// 受け取った書き込みを記録し、PLC の応答待ちに見立てて遅らせてから Good を返す。
    struct RecordingSink(mpsc::UnboundedSender<WriteAttempt>);

    impl WriteSink for RecordingSink {
        fn write(&self, attempt: WriteAttempt) -> WriteFuture {
            let sender = self.0.clone();
            Box::pin(async move {
                tokio::time::sleep(WRITE_DELAY).await;
                sender.send(attempt).expect("書き込みを記録する");
                StatusCode::Good
            })
        }
    }

    async fn connect(server: &RunningServer) -> (std::sync::Arc<Session>, tempfile::TempDir) {
        let pki = tempfile::tempdir().expect("一時 PKI を作る");
        let mut client = ClientBuilder::new()
            .application_name("banto-opcua-write-test")
            .application_uri("urn:banto:opcua:write-test")
            .create_sample_keypair(true)
            .pki_dir(pki.path())
            .trust_server_certs(true)
            .verify_server_certs(false)
            .client()
            .expect("クライアントを作る");
        let (session, event_loop) = client
            .connect_to_matching_endpoint(
                (
                    server.endpoint(),
                    SecurityPolicy::None.to_str(),
                    MessageSecurityMode::None,
                    UserTokenPolicy::anonymous(),
                ),
                IdentityToken::Anonymous,
            )
            .await
            .expect("エンドポイントに接続する");
        event_loop.spawn();
        assert!(
            tokio::time::timeout(Duration::from_secs(10), session.wait_for_connection())
                .await
                .expect("接続待ちが時間切れ")
        );
        (session, pki)
    }

    /// 試作のため、AccessLevel をアドレス空間で直接書き込み可にする。
    fn make_writable(server: &RunningServer, id: &NodeId) {
        let mut space = server.fixed_tags.manager.address_space().write();
        let Some(NodeType::Variable(variable)) = space.find_mut(id) else {
            panic!("変数 {id} がある");
        };
        let level = AccessLevel::CURRENT_READ | AccessLevel::CURRENT_WRITE;
        variable.set_access_level(level);
        variable.set_user_access_level(level);
    }

    fn value_write(id: &NodeId, value: DataValue) -> WriteValue {
        WriteValue {
            node_id: id.clone(),
            attribute_id: AttributeId::Value as u32,
            index_range: Default::default(),
            value,
        }
    }

    async fn read_value(session: &Session, id: &NodeId) -> Option<Variant> {
        session
            .read(
                &[ReadValueId::from(id.clone())],
                TimestampsToReturn::Both,
                0.0,
            )
            .await
            .expect("Read する")
            .remove(0)
            .value
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn write_reaches_the_sink_without_blocking_reads() {
        let (sender, mut written) = mpsc::unbounded_channel();
        let server = OpcUaServer::start_with(
            ServerConfig::default().with_port(0),
            Some(Arc::new(RecordingSink(sender))),
        )
        .await
        .expect("サーバーを起動する");
        server
            .add_tag("PLC2", "Cell1", "Speed", MappedValue::Double(1.0))
            .expect("タグを登録する");
        let ns = server.namespace_index();
        let speed = NodeId::new(ns, "PLC2.Cell1.Speed");
        let sample = NodeId::new(ns, "PLC1.Line1.Speed");
        make_writable(&server, &speed);
        let (session, _pki) = connect(&server).await;

        // asyncua と同じく SourceTimestamp を付けて書く。時刻は捨てて受け付ける。
        let timed = DataValue {
            value: Some(Variant::Double(5.0)),
            source_timestamp: Some(DateTime::now()),
            ..Default::default()
        };
        let writer = {
            let session = session.clone();
            let request = value_write(&speed, timed);
            tokio::spawn(async move { session.write(&[request]).await })
        };
        // 書き込み先が応答を待っている間も、Read は待たされない。
        tokio::time::sleep(Duration::from_millis(50)).await;
        let started = Instant::now();
        assert_eq!(
            read_value(&session, &speed).await,
            Some(Variant::Double(1.0))
        );
        assert!(
            started.elapsed() < WRITE_DELAY,
            "Read は書き込みの完了を待たない"
        );
        let results = writer.await.expect("書き込みタスク").expect("Write する");
        assert_eq!(results, vec![StatusCode::Good]);
        assert_eq!(
            written.recv().await,
            Some(WriteAttempt {
                node_id: speed.clone(),
                value: Variant::Double(5.0),
                anonymous: true,
            })
        );
        assert_eq!(
            read_value(&session, &speed).await,
            Some(Variant::Double(1.0)),
            "Write だけではノードの値を変えない"
        );

        let rejected = [
            // AccessLevel が CurrentRead のみのサンプルタグ。
            value_write(&sample, DataValue::new_now(Variant::Double(2.0))),
            // Double のノードへ Boolean。
            value_write(&speed, DataValue::new_now(Variant::Boolean(true))),
            // IndexRange 付き。
            WriteValue {
                index_range: "0".parse().expect("IndexRange"),
                ..value_write(&speed, DataValue::new_now(Variant::Double(2.0)))
            },
            // Bad の StatusCode 付き。
            value_write(
                &speed,
                DataValue {
                    value: Some(Variant::Double(2.0)),
                    status: Some(StatusCode::Bad),
                    ..Default::default()
                },
            ),
        ];
        assert_eq!(
            session.write(&rejected).await.expect("Write する"),
            vec![
                StatusCode::BadNotWritable,
                StatusCode::BadTypeMismatch,
                StatusCode::BadIndexRangeNoData,
                StatusCode::BadWriteNotSupported,
            ]
        );
        assert!(
            written.try_recv().is_err(),
            "拒否した書き込みは書き込み先へ渡さない"
        );
        server.shutdown().await.expect("サーバーを停止する");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn without_a_sink_every_write_is_not_writable() {
        let server = OpcUaServer::start(ServerConfig::default().with_port(0))
            .await
            .expect("サーバーを起動する");
        server
            .add_tag("PLC2", "Cell1", "Speed", MappedValue::Double(1.0))
            .expect("タグを登録する");
        let speed = NodeId::new(server.namespace_index(), "PLC2.Cell1.Speed");
        make_writable(&server, &speed);
        let (session, _pki) = connect(&server).await;
        assert_eq!(
            session
                .write(&[value_write(
                    &speed,
                    DataValue::new_now(Variant::Double(2.0))
                )])
                .await
                .expect("Write する"),
            vec![StatusCode::BadNotWritable]
        );
        server.shutdown().await.expect("サーバーを停止する");
    }
}
