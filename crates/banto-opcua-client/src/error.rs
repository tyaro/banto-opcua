/// クライアントの接続と Read で起きる失敗。
#[derive(Debug, thiserror::Error)]
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
    /// Read サービスが失敗した、または値が想定と違った。
    #[error("OPC UA の Read に失敗しました: {0}")]
    Read(String),
}
