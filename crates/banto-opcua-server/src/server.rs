//! async-opcua の `ServerBuilder` を、ループバック既定の待受で包む。
//!
//! タグノードの追加はまだしない。標準名前空間が載るところまでがこの足場の範囲。

use std::net::SocketAddr;

use opcua::server::{ANONYMOUS_USER_TOKEN_ID, ServerBuilder, ServerEndpoint, ServerHandle};
use tokio::{net::TcpListener, task::JoinHandle};

use crate::{Error, ServerConfig, endpoint_url};

/// サーバーを起動する入口。状態は持たない。
#[derive(Debug, Default)]
pub struct OpcUaServer;

impl OpcUaServer {
    /// 設定のアドレスで待受を始める。ポート `0` のときは空きポートを使う。
    ///
    /// 返した [`RunningServer`] を落とすと待受を止める。明示的に止めるときは
    /// [`RunningServer::shutdown`]。
    pub async fn start(config: ServerConfig) -> Result<RunningServer, Error> {
        let pki = tempfile::tempdir().map_err(Error::Pki)?;
        let listener = TcpListener::bind((config.bind, config.port))
            .await
            .map_err(Error::Bind)?;
        let local_addr = listener.local_addr().map_err(Error::Bind)?;
        let path = normalize_path(&config.endpoint_path);
        let endpoint = endpoint_url(local_addr, &path);

        let (server, handle) = ServerBuilder::new()
            .application_name(config.application_name)
            .application_uri(config.application_uri)
            .product_uri(config.product_uri)
            .create_sample_keypair(true)
            .certificate_path("own/cert.der")
            .private_key_path("private/private.pem")
            .pki_dir(pki.path())
            .host(local_addr.ip().to_string())
            .port(local_addr.port())
            .trust_client_certs(true)
            .add_endpoint(
                "none",
                ServerEndpoint::new_none(&path, &[ANONYMOUS_USER_TOKEN_ID.to_owned()]),
            )
            .discovery_urls(vec![endpoint.clone()])
            .build()
            .map_err(Error::Build)?;

        let task = tokio::spawn(async move { server.run_with(listener).await });

        Ok(RunningServer {
            local_addr,
            endpoint,
            handle,
            task: Some(task),
            _pki: pki,
        })
    }
}

/// 起動済みのサーバー。ドロップするとキャンセルする。
#[must_use = "シャットダウンするまで RunningServer を保持する"]
pub struct RunningServer {
    local_addr: SocketAddr,
    endpoint: String,
    handle: ServerHandle,
    task: Option<JoinHandle<Result<(), String>>>,
    _pki: tempfile::TempDir,
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

    /// クライアントへ切断を知らせてからタスクの終了を待つ。
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
    }
}

/// エンドポイントパスを `/` 始まりにそろえる。空は `/`。
fn normalize_path(endpoint_path: &str) -> String {
    if endpoint_path.is_empty() || endpoint_path == "/" {
        "/".to_owned()
    } else if let Some(stripped) = endpoint_path.strip_prefix('/') {
        format!("/{stripped}")
    } else {
        format!("/{endpoint_path}")
    }
}
