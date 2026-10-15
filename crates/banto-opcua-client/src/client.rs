//! `ClientBuilder` で None / Anonymous のセッションを張り、標準ノードを読む。

use std::{sync::Arc, time::Duration};

use opcua::{
    client::{Client, ClientBuilder, IdentityToken, Session},
    crypto::SecurityPolicy,
    types::{
        AttributeId, MessageSecurityMode, NodeId as UaNodeId, ReadValueId, StatusCode,
        TimestampsToReturn, UserTokenPolicy, VariableId, Variant,
    },
};
use tokio::task::JoinHandle;

use crate::Error;
use banto_opcua_core::{Identifier, NodeId};

/// クライアントを接続する入口。状態は持たない。
#[derive(Debug, Default)]
pub struct OpcUaClient;

impl OpcUaClient {
    /// `opc.tcp://` URL へ SecurityPolicy `None`、Anonymous で接続する。
    ///
    /// `verify_server_certs(false)` はホスト名検証だけでなく、サーバー証明書の
    /// 検証全体を無効にする。Phase 1 はループバックの平文試験であり、サンプル
    /// 証明書では検証を通せない。Phase 3 TODO: 証明書検証を戻す。
    pub async fn connect(endpoint: &str) -> Result<ConnectedClient, Error> {
        Self::connect_with_session_timeout(endpoint, Duration::from_secs(20)).await
    }

    /// エンドポイントの取得後、セッション確立を待つ時間を指定して接続する。
    ///
    /// 時間切れや呼び出し元によるキャンセルではイベントループを中止する。
    /// エンドポイントの取得処理にはこの時間制限を適用しない。
    pub async fn connect_with_session_timeout(
        endpoint: &str,
        session_timeout: Duration,
    ) -> Result<ConnectedClient, Error> {
        let pki = tempfile::tempdir().map_err(Error::Pki)?;
        let mut client = ClientBuilder::new()
            .application_name("banto-opcua-client")
            .application_uri("urn:banto:opcua:client")
            .product_uri("urn:banto:opcua")
            .create_sample_keypair(true)
            .pki_dir(pki.path())
            .trust_server_certs(true)
            // 証明書検証全体を無効化する。ホスト名だけのスイッチではない。
            // Phase 3 TODO: 検証を戻す。
            .verify_server_certs(false)
            .session_retry_limit(2)
            .client()
            .map_err(|reasons| Error::Build(reasons.join("; ")))?;

        let (session, event_loop) = client
            .connect_to_matching_endpoint(
                (
                    endpoint,
                    SecurityPolicy::None.to_str(),
                    MessageSecurityMode::None,
                    UserTokenPolicy::anonymous(),
                ),
                IdentityToken::Anonymous,
            )
            .await
            .map_err(|err| Error::Connect(err.to_string()))?;

        // 時間切れや未接続で抜けると、このガードの Drop がタスクを abort する。
        // 成功したときだけ disarm して ConnectedClient へ所有権を渡す。
        let task = AbortOnDrop::new(event_loop.spawn());
        let connected = tokio::time::timeout(session_timeout, session.wait_for_connection())
            .await
            .map_err(|_| Error::Connect("接続待ちが時間切れになりました".to_owned()))?;
        if !connected {
            return Err(Error::Connect("セッションが確立しませんでした".to_owned()));
        }

        Ok(ConnectedClient {
            session,
            task: Some(task.disarm()),
            _client: client,
            _pki: pki,
        })
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
#[must_use = "切断するまで ConnectedClient を保持する"]
pub struct ConnectedClient {
    session: Arc<Session>,
    task: Option<JoinHandle<StatusCode>>,
    _client: Client,
    _pki: tempfile::TempDir,
}

impl ConnectedClient {
    /// `ServerStatus/State` を読む。OPC UA では Running が 0。
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
    /// 値が無い、または Bad の場合はエラーにする。
    pub async fn read_u32(&self, node_id: &NodeId) -> Result<u32, Error> {
        let id = match &node_id.identifier {
            Identifier::Numeric(id) => UaNodeId::new(node_id.namespace, *id),
            Identifier::String(id) => UaNodeId::new(node_id.namespace, id.clone()),
        };
        let value = self.read_value(id).await?;
        match value {
            Variant::UInt32(number) => Ok(number),
            other => Err(Error::Read(format!(
                "{node_id} の型が UInt32 ではありません: {other:?}"
            ))),
        }
    }

    async fn read_value(&self, node_id: UaNodeId) -> Result<Variant, Error> {
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
            .map_err(|err| Error::Read(err.to_string()))?;
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
    pub async fn disconnect(mut self) -> Result<(), Error> {
        self.session
            .disconnect()
            .await
            .map_err(|err| Error::Connect(err.to_string()))?;
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
        Ok(())
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
}
