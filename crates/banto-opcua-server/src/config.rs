//! サーバーの待受設定。証明書とセキュリティポリシーの項目は Phase 3 まで置かない。

use std::net::{IpAddr, Ipv4Addr};

/// OPC UA サーバーの待受設定。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerConfig {
    /// 待受アドレス。既定は `127.0.0.1`。
    pub bind: IpAddr,
    /// 待受ポート。`0` は OS が選ぶ空きポート（テスト用）。既定は 4840。
    pub port: u16,
    /// エンドポイントのパス。`/` から始める。
    pub endpoint_path: String,
    /// ApplicationName。
    pub application_name: String,
    /// ApplicationUri。証明書の URI と揃えるのは Phase 3。
    pub application_uri: String,
    /// ProductUri。
    pub product_uri: String,
    /// タグを載せる予定の名前空間 URI。この足場ではまだ登録しない。
    pub namespace_uri: String,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bind: IpAddr::V4(Ipv4Addr::LOCALHOST),
            port: 4840,
            endpoint_path: "/".to_owned(),
            application_name: "banto-opcua".to_owned(),
            application_uri: "urn:banto:opcua:server".to_owned(),
            product_uri: "urn:banto:opcua".to_owned(),
            namespace_uri: "urn:banto:opcua:tags".to_owned(),
        }
    }
}

impl ServerConfig {
    /// 実際に bind したアドレスから、クライアントが使う URL を作る。
    pub fn endpoint_url(&self, bound: std::net::SocketAddr) -> String {
        endpoint_url(bound, &self.endpoint_path)
    }
}

/// `opc.tcp://` URL を組み立てる。IPv6 は括弧で囲む。
pub fn endpoint_url(bound: std::net::SocketAddr, endpoint_path: &str) -> String {
    let path = if endpoint_path.is_empty() {
        "/".to_owned()
    } else if endpoint_path.starts_with('/') {
        endpoint_path.to_owned()
    } else {
        format!("/{endpoint_path}")
    };
    match bound.ip() {
        IpAddr::V4(ip) => format!("opc.tcp://{ip}:{}{path}", bound.port()),
        IpAddr::V6(ip) => format!("opc.tcp://[{ip}]:{}{path}", bound.port()),
    }
}

#[cfg(test)]
mod tests {
    use std::net::{Ipv6Addr, SocketAddr};

    use super::*;

    #[test]
    fn default_bind_is_loopback_only() {
        let config = ServerConfig::default();
        assert_eq!(config.bind, IpAddr::V4(Ipv4Addr::LOCALHOST));
        assert_eq!(config.port, 4840);
        assert_eq!(config.endpoint_path, "/");
    }

    #[test]
    fn endpoint_url_uses_the_bound_port() {
        let bound = SocketAddr::from((Ipv4Addr::LOCALHOST, 4840));
        assert_eq!(endpoint_url(bound, "/"), "opc.tcp://127.0.0.1:4840/");
        let ephemeral = SocketAddr::from((Ipv4Addr::LOCALHOST, 49152));
        assert_eq!(
            endpoint_url(ephemeral, "banto"),
            "opc.tcp://127.0.0.1:49152/banto"
        );
    }

    #[test]
    fn ipv6_endpoint_url_is_bracketed() {
        let bound = SocketAddr::from((Ipv6Addr::LOCALHOST, 4840));
        assert_eq!(endpoint_url(bound, "/"), "opc.tcp://[::1]:4840/");
    }
}
