//! サーバーの待受設定。SecurityPolicy と認証の項目は Phase 3 まで置かない。
//! 例外として、自己署名の鍵と証明書の保存先だけを [`ServerConfig::pki_dir`] に
//! 置く（[ADR-0003](https://github.com/tyaro/banto-opcua/blob/main/docs/adr/0003-persistent-pki-dir.md)）。

use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::PathBuf,
};

/// OPC UA サーバーの待受設定。
///
/// 外部クレートでは [`Default`] と `with_*` メソッドで生成する。
/// 新しい設定項目が増えても構造体リテラルを更新せずに済む。
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
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
    /// Phase 1 の固定タグを登録する名前空間 URI。空文字と標準・アプリケーション
    /// 名前空間との重複は起動時に拒否する。async-opcua 0.19 が自動登録するのは
    /// 標準 URI と ApplicationUri の 2 つで、固定タグはそれらと別の URI を使う。
    pub namespace_uri: String,
    /// Phase 1 のサンプルタグを登録する。ハブの実タグを公開するときは無効にする。
    pub sample_tags: bool,
    /// エンドポイント URL と discovery URL に載せるホスト。
    ///
    /// 未設定なら bind した具体的なアドレスを載せる。未指定アドレス
    /// （`0.0.0.0` / `::`）で待受するときは必須。IPv6 は括弧なしでもよい。
    /// DNS 名の末尾のドットは URL に載せるときに除く。
    pub advertised_host: Option<String>,
    /// サーバーの自己署名証明書と秘密鍵を置くディレクトリ。
    ///
    /// 未設定なら起動ごとに一時ディレクトリへ RSA 鍵を作り、停止で消す。
    /// 設定すると、そこに鍵と証明書が無いときだけ作り、以降の起動では
    /// 再利用する。ディレクトリが無ければ作る。片方だけ残っていたら消して
    /// 組で作り直す。存在を確かめられない（権限エラーなど）ときは何も消さず、
    /// 起動を止める。書き込めないなどで組を作れなかったときも起動を止める。
    /// 壊れたファイルは検証しないので、そのときは両方を消す。
    /// SecurityPolicy は `None` のままで、鍵は通信の暗号化に使わない。
    ///
    /// Unix では鍵の生成前に秘密鍵のディレクトリを所有者だけのものにし、
    /// 生成後に秘密鍵も所有者だけが読めるようにする。Windows の ACL は
    /// 呼び出し側のデータディレクトリに従う。同時に起動する複数のサーバーで
    /// 同じディレクトリを共有しない（初回の生成が競合する）。
    pub pki_dir: Option<PathBuf>,
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
            sample_tags: true,
            advertised_host: None,
            pki_dir: None,
        }
    }
}

impl ServerConfig {
    /// 待受 IP アドレスを設定する。
    pub fn with_bind(mut self, bind: IpAddr) -> Self {
        self.bind = bind;
        self
    }

    /// 待受ポートを設定する。
    pub fn with_port(mut self, port: u16) -> Self {
        self.port = port;
        self
    }

    /// エンドポイントのパスを設定する。
    pub fn with_endpoint_path(mut self, path: impl Into<String>) -> Self {
        self.endpoint_path = path.into();
        self
    }

    /// ApplicationName を設定する。
    pub fn with_application_name(mut self, name: impl Into<String>) -> Self {
        self.application_name = name.into();
        self
    }

    /// ApplicationUri を設定する。
    pub fn with_application_uri(mut self, uri: impl Into<String>) -> Self {
        self.application_uri = uri.into();
        self
    }

    /// ProductUri を設定する。
    pub fn with_product_uri(mut self, uri: impl Into<String>) -> Self {
        self.product_uri = uri.into();
        self
    }

    /// タグの名前空間 URI を設定する。
    pub fn with_namespace_uri(mut self, uri: impl Into<String>) -> Self {
        self.namespace_uri = uri.into();
        self
    }

    /// Phase 1 のサンプルタグを登録するか選ぶ。
    pub fn with_sample_tags(mut self, enabled: bool) -> Self {
        self.sample_tags = enabled;
        self
    }

    /// エンドポイント URL と discovery URL の公開ホストを設定する。
    /// 形式は [`crate::OpcUaServer::start`] で検証する。
    pub fn with_advertised_host(mut self, host: impl Into<String>) -> Self {
        self.advertised_host = Some(host.into());
        self
    }

    /// 自己署名の鍵と証明書を再利用するディレクトリを設定する。
    pub fn with_pki_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.pki_dir = Some(dir.into());
        self
    }

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

/// `advertised_host` が単体のホスト名または IP アドレスか。
/// URL、ポート、パス、空白、IPv6 の不完全な括弧を含めない。
pub(crate) fn valid_advertised_host(host: &str) -> bool {
    if host.is_empty() || host != host.trim() {
        return false;
    }
    if let Some(bare) = host.strip_prefix('[') {
        return bare
            .strip_suffix(']')
            .is_some_and(|ip| ip.parse::<std::net::Ipv6Addr>().is_ok());
    }
    if host.parse::<IpAddr>().is_ok() {
        return true;
    }
    let dns_name = host.strip_suffix('.').unwrap_or(host);
    if dns_name.len() > 253 || !dns_name.is_ascii() {
        return false;
    }
    dns_name.split('.').all(|label| {
        !label.is_empty()
            && label.len() <= 63
            && label
                .as_bytes()
                .first()
                .is_some_and(u8::is_ascii_alphanumeric)
            && label
                .as_bytes()
                .last()
                .is_some_and(u8::is_ascii_alphanumeric)
            && label
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    })
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
        host.strip_suffix('.').unwrap_or(host).to_owned()
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
    bare.strip_suffix('.')
        .unwrap_or(bare)
        .eq_ignore_ascii_case("localhost")
}

fn ip_from_host(host: &str) -> Option<IpAddr> {
    let host = host.trim();
    let bare = host
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
        .unwrap_or(host);
    bare.strip_suffix('.').unwrap_or(bare).parse().ok()
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
        assert_eq!(config.pki_dir, None);
    }

    #[test]
    fn endpoint_url_uses_the_bound_port() {
        let bound = SocketAddr::from((Ipv4Addr::LOCALHOST, 4840));
        let config = ServerConfig::default();
        assert_eq!(config.endpoint_url(bound), "opc.tcp://127.0.0.1:4840/");
        let ephemeral = SocketAddr::from((Ipv4Addr::LOCALHOST, 49152));
        assert_eq!(
            ServerConfig {
                endpoint_path: "banto".to_owned(),
                ..config
            }
            .endpoint_url(ephemeral),
            "opc.tcp://127.0.0.1:49152/banto"
        );
    }

    #[test]
    fn endpoint_path_is_normalized_in_one_place() {
        let bound = SocketAddr::from((Ipv4Addr::LOCALHOST, 4840));
        let mut config = ServerConfig::default().with_endpoint_path("banto");
        let plain = config.endpoint_url(bound);
        config.endpoint_path = "/banto".to_owned();
        assert_eq!(plain, config.endpoint_url(bound));
        config.endpoint_path = "".to_owned();
        let empty = config.endpoint_url(bound);
        config.endpoint_path = "/".to_owned();
        assert_eq!(empty, config.endpoint_url(bound));
    }

    #[test]
    fn ipv6_endpoint_url_is_bracketed() {
        let bound = SocketAddr::from((Ipv6Addr::LOCALHOST, 4840));
        assert_eq!(
            ServerConfig::default().endpoint_url(bound),
            "opc.tcp://[::1]:4840/"
        );
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

    #[test]
    fn advertised_host_accepts_hostnames_and_ip_addresses_only() {
        for host in [
            "localhost",
            "plc.example.com",
            "plc.example.com.",
            "192.0.2.10",
            "::1",
            "[::1]",
        ] {
            assert!(valid_advertised_host(host), "{host}");
        }
        for host in [
            "",
            " plc.example.com",
            "plc.example.com ",
            "a b",
            "opc.tcp://host",
            "host:4840",
            "host/path",
            "[::1",
            "[host]",
            "bad_host",
            "-host",
            "host-",
            "host..example",
            "plc.example.com..",
            "[::1].",
            "host:foo",
        ] {
            assert!(!valid_advertised_host(host), "{host}");
        }
        let config = ServerConfig::default().with_advertised_host("plc.example.com.");
        let bound = SocketAddr::from((Ipv4Addr::LOCALHOST, 4840));
        assert_eq!(
            config.endpoint_url(bound),
            "opc.tcp://plc.example.com:4840/"
        );
        assert!(host_is_unspecified("0.0.0.0."));
        assert!(host_is_loopback("localhost."));
    }
}
