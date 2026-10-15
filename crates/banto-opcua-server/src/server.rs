//! async-opcua の `ServerBuilder` を、ループバック既定の待受で包む。
//!
//! タグノードの追加はまだしない。標準名前空間が載るところまでがこの足場の範囲。

use std::net::SocketAddr;

use opcua::server::{ANONYMOUS_USER_TOKEN_ID, ServerBuilder, ServerEndpoint, ServerHandle};
use tokio::{net::TcpListener, task::JoinHandle};

use crate::{
    Error, ServerConfig,
    config::{
        host_is_loopback, host_is_unspecified, normalize_endpoint_path, valid_advertised_host,
    },
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
    pub async fn start(config: ServerConfig) -> Result<RunningServer, Error> {
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

        let pki = tempfile::tempdir().map_err(Error::Pki)?;
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

        let (server, handle) = ServerBuilder::new()
            .application_name(config.application_name)
            .application_uri(config.application_uri)
            .product_uri(config.product_uri)
            .create_sample_keypair(true)
            .certificate_path("own/cert.der")
            .private_key_path("private/private.pem")
            .pki_dir(pki.path())
            .host(endpoint_host)
            .port(local_addr.port())
            .trust_client_certs(true)
            .add_endpoint(
                "none",
                ServerEndpoint::new_none(&path, &[ANONYMOUS_USER_TOKEN_ID.to_owned()]),
            )
            .discovery_urls(vec![endpoint.clone()])
            .build()
            .map_err(Error::Build)?;

        // 一時ディレクトリはタスクが所有する。Drop が先に消すと、停止前の
        // サーバーが証明書を読めなくなる。
        let task = tokio::spawn(async move {
            let _pki = pki;
            server.run_with(listener).await
        });

        Ok(RunningServer {
            local_addr,
            endpoint,
            handle,
            task: Some(task),
        })
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
    use std::net::{IpAddr, Ipv4Addr};

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
}
