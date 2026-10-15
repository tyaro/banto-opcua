//! `ClientBuilder` で None / Anonymous のセッションを張り、標準ノードを読む。

use std::{sync::Arc, time::Duration};

use opcua::{
    client::{Client, ClientBuilder, IdentityToken, Session},
    crypto::SecurityPolicy,
    types::{
        AttributeId, MessageSecurityMode, ReadValueId, StatusCode, TimestampsToReturn,
        UserTokenPolicy, VariableId, Variant,
    },
};
use tokio::task::JoinHandle;

use crate::Error;

/// クライアントを接続する入口。状態は持たない。
#[derive(Debug, Default)]
pub struct OpcUaClient;

impl OpcUaClient {
    /// `opc.tcp://` URL へ SecurityPolicy `None`、Anonymous で接続する。
    ///
    /// 証明書のホスト名検証は切っている。Phase 1 はループバックの平文試験であり、
    /// サンプル証明書の SAN と `127.0.0.1` は一致しない。Phase 3 で検証を戻す。
    pub async fn connect(endpoint: &str) -> Result<ConnectedClient, Error> {
        let pki = tempfile::tempdir().map_err(Error::Pki)?;
        let mut client = ClientBuilder::new()
            .application_name("banto-opcua-client")
            .application_uri("urn:banto:opcua:client")
            .product_uri("urn:banto:opcua")
            .create_sample_keypair(true)
            .pki_dir(pki.path())
            .trust_server_certs(true)
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

        let task = event_loop.spawn();
        let connected =
            tokio::time::timeout(Duration::from_secs(20), session.wait_for_connection())
                .await
                .map_err(|_| Error::Connect("接続待ちが時間切れになりました".to_owned()))?;
        if !connected {
            return Err(Error::Connect("セッションが確立しませんでした".to_owned()));
        }

        Ok(ConnectedClient {
            session,
            task: Some(task),
            _client: client,
            _pki: pki,
        })
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
        let values = self
            .session
            .read(
                &[ReadValueId {
                    node_id: VariableId::Server_ServerStatus_State.into(),
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
        match value.value {
            Some(Variant::Int32(state)) => Ok(state),
            Some(other) => Err(Error::Read(format!(
                "State の型が Int32 ではありません: {other:?}"
            ))),
            None => Err(Error::Read("State の値がありません".to_owned())),
        }
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
