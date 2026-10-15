/// サーバーの起動と停止で起きる失敗。
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// 未指定アドレスをエンドポイントへ載せる設定になっている。
    #[error(
        "未指定アドレス（0.0.0.0 / ::）はエンドポイントに載せられません。公開するホストを advertised_host に設定してください"
    )]
    AdvertisedHostRequired,
    /// TCP の bind、またはローカルアドレスの取得に失敗した。
    #[error("OPC UA サーバーの待受に失敗しました: {0}")]
    Bind(#[source] std::io::Error),
    /// サンプル証明書を置く一時ディレクトリを作れなかった。
    #[error("OPC UA サーバーの一時ディレクトリを用意できませんでした: {0}")]
    Pki(#[source] std::io::Error),
    /// async-opcua の設定検証に失敗した。
    #[error("OPC UA サーバーの構築に失敗しました: {0}")]
    Build(String),
    /// サーバータスクがエラーで戻った、またはパニックした。
    #[error("OPC UA サーバーの実行に失敗しました: {0}")]
    Runtime(String),
}
