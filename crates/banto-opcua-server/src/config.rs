//! サーバーの待受設定。証明書とセキュリティポリシーの項目は Phase 3 まで置かない。

use std::net::{IpAddr, Ipv4Addr, SocketAddr};

/// OPC UA サーバーの待受設定。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerConfig {
    /// 待受アドレス。既定は `127.0.0.1`。
    ///
    /// `0.0.0.0` や `::` で待受するときは [`Self::advertised_host`] が必須。
    pub bind: IpAddr,
    /// 待受ポート。`0` は OS が選ぶ空きポート（テスト用）。既定は 4840。
    pub port: u16,
    /// エンドポイントのパス。先頭の `/` が無ければ URL 生成時に補う。空は `/`。
    pub endpoint_path: String,
    /// ApplicationName。
    pub application_name: String,
    /// ApplicationUri。証明書の URI と揃えるのは Phase 3。
    pub application_uri: String,
    /// ProductUri。
    pub product_uri: String,
    /// タグを載せる予定の名前空間 URI。この足場ではまだ登録しない。
    pub namespace_uri: String,
    /// エンドポイント URL と discovery URL に載せるホスト。
    ///
    /// 未設定なら bind した具体的なアドレスを載せる。未指定アドレス
    /// （`0.0.0.0` / `::`）で待受するときは必須。IPv6 は括弧なしでもよい。
    pub advertised_host: Option<String>,
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
            advertised_host: None,
        }
    }
}

impl ServerConfig {
    /// 空や空白だけの `advertised_host` は未設定として扱う。
    pub(crate) fn explicit_advertised_host(&self) -> Option<&str> {
        self.advertised_host
            .as_deref()
            .map(str::trim)
            .filter(|host| !host.is_empty())
    }

    /// エンドポイント URL に載せるホスト。IPv6 は括弧付き。
    pub(crate) fn endpoint_host(&self, bound: SocketAddr) -> String {
        let raw = self
            .explicit_advertised_host()
            .map(str::to_owned)
            .unwrap_or_else(|| bound.ip().to_string());
        format_endpoint_host(&raw)
    }

    /// 実際に bind したポートと、公開するホストから URL を作る。
    ///
    /// `advertised_host` があればそれを使う。無ければ `bound` の IP を使う。
    pub fn endpoint_url(&self, bound: SocketAddr) -> String {
        format!(
            "opc.tcp://{}:{}{}",
            self.endpoint_host(bound),
            bound.port(),
            normalize_endpoint_path(&self.endpoint_path)
        )
    }
}

/// `opc.tcp://` URL を組み立てる。IPv6 は括弧で囲む。
///
/// `advertised_host` を反映するのは [`ServerConfig::endpoint_url`]。
pub fn endpoint_url(bound: SocketAddr, endpoint_path: &str) -> String {
    format!(
        "opc.tcp://{}:{}{}",
        format_endpoint_host(&bound.ip().to_string()),
        bound.port(),
        normalize_endpoint_path(endpoint_path)
    )
}

/// ホスト文字列をエンドポイント URL 用に整える。IPv6 は `[..]` で囲む。
pub(crate) fn format_endpoint_host(host: &str) -> String {
    let host = host.trim();
    let bare = host
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
        .unwrap_or(host);
    if bare.parse::<std::net::Ipv6Addr>().is_ok() {
        format!("[{bare}]")
    } else {
        host.to_owned()
    }
}

/// エンドポイントパスを `/` 始まりにそろえる。空は `/`。
pub(crate) fn normalize_endpoint_path(endpoint_path: &str) -> String {
    if endpoint_path.is_empty() || endpoint_path == "/" {
        "/".to_owned()
    } else if let Some(stripped) = endpoint_path.strip_prefix('/') {
        format!("/{stripped}")
    } else {
        format!("/{endpoint_path}")
    }
}

/// ホストが未指定アドレス（`0.0.0.0` / `::`）か。括弧付き IPv6 も見る。
pub(crate) fn host_is_unspecified(host: &str) -> bool {
    ip_from_host(host).is_some_and(|ip| ip.is_unspecified())
}

/// ホストがループバックか。`localhost` もループバックとして扱う。
pub(crate) fn host_is_loopback(host: &str) -> bool {
    if ip_from_host(host).is_some_and(|ip| ip.is_loopback()) {
        return true;
    }
    let bare = host.trim();
    let bare = bare
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
        .unwrap_or(bare);
    bare.eq_ignore_ascii_case("localhost")
}

fn ip_from_host(host: &str) -> Option<IpAddr> {
    let host = host.trim();
    let bare = host
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
        .unwrap_or(host);
    bare.parse().ok()
}

#[cfg(test)]
mod tests {
    use std::net::Ipv6Addr;

    use super::*;

    #[test]
    fn default_bind_is_loopback_only() {
        let config = ServerConfig::default();
        assert_eq!(config.bind, IpAddr::V4(Ipv4Addr::LOCALHOST));
        assert_eq!(config.port, 4840);
        assert_eq!(config.endpoint_path, "/");
        assert_eq!(config.advertised_host, None);
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
    fn endpoint_path_is_normalized_in_one_place() {
        let bound = SocketAddr::from((Ipv4Addr::LOCALHOST, 4840));
        assert_eq!(endpoint_url(bound, "banto"), endpoint_url(bound, "/banto"));
        assert_eq!(endpoint_url(bound, ""), endpoint_url(bound, "/"));
    }

    #[test]
    fn ipv6_endpoint_url_is_bracketed() {
        let bound = SocketAddr::from((Ipv6Addr::LOCALHOST, 4840));
        assert_eq!(endpoint_url(bound, "/"), "opc.tcp://[::1]:4840/");
    }

    #[test]
    fn advertised_host_overrides_the_bound_ip_and_brackets_ipv6() {
        let bound = SocketAddr::from((Ipv4Addr::UNSPECIFIED, 4840));
        let config = ServerConfig {
            advertised_host: Some("::1".to_owned()),
            ..ServerConfig::default()
        };
        assert_eq!(config.endpoint_url(bound), "opc.tcp://[::1]:4840/");
        assert_eq!(config.endpoint_host(bound), "[::1]");

        let bracketed = ServerConfig {
            advertised_host: Some("[2001:db8::1]".to_owned()),
            endpoint_path: "banto".to_owned(),
            ..ServerConfig::default()
        };
        assert_eq!(
            bracketed.endpoint_url(bound),
            "opc.tcp://[2001:db8::1]:4840/banto"
        );
        assert_eq!(format_endpoint_host("[::1]"), "[::1]");
        assert_eq!(format_endpoint_host("localhost"), "localhost");
    }

    #[test]
    fn unspecified_and_loopback_hosts_are_classified() {
        assert!(host_is_unspecified("0.0.0.0"));
        assert!(host_is_unspecified("::"));
        assert!(host_is_unspecified("[::]"));
        assert!(!host_is_unspecified("127.0.0.1"));
        assert!(host_is_loopback("127.0.0.1"));
        assert!(host_is_loopback("[::1]"));
        assert!(host_is_loopback("localhost"));
        assert!(!host_is_loopback("192.0.2.10"));
    }
}
