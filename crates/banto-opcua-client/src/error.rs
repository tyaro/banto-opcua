/// クライアントの接続と Read / Write で起きる失敗。
///
/// [`crate::ConnectedClient::read_batch`] の `Err` と [`crate::WriteFailure::error`] は
/// どれも接続ごとの失敗（ノード単位の失敗は結果の StatusCode）。
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// サンプル証明書を置く一時ディレクトリを作れなかった。
    #[error("OPC UA クライアントの一時ディレクトリを用意できませんでした: {0}")]
    Pki(#[source] std::io::Error),
    /// クライアント設定の検証に失敗した。
    #[error("OPC UA クライアントの構築に失敗しました: {0}")]
    Build(String),
    /// セッション確立前の失敗、または接続待ちの時間切れ。
    #[error("OPC UA サーバーへ接続できませんでした: {0}")]
    Connect(String),
    /// 接続が切れている（自動再接続のときは張り直している最中）。
    #[error("OPC UA サーバーとの接続が切れています")]
    Disconnected,
    /// 購読や MonitoredItem を作れなかった、または購読を失った（ADR-0006）。
    #[error("OPC UA の購読に失敗しました: {0}")]
    Subscribe(String),
    /// Write の要求が拒否された、または結果を受け取れなかった。
    #[error("OPC UA の Write に失敗しました: {0}")]
    Write(String),
    /// Read サービスが失敗した、または値が想定と違った。
    #[error("OPC UA の Read に失敗しました: {0}")]
    Read(String),
}
