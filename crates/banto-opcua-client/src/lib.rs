//! banto 向け OPC UA クライアントの足場。
//!
//! 最初の利用者はサーバークレートのプロセス内テスト。製品としてのクライアント
//! （ハブが他の OPC UA サーバーを読む）は tyaro/banto-industrial#201 の対象外で、
//! 別 issue。ここにあるのは SecurityPolicy `None` / Anonymous で接続し、
//! 標準ノード `ServerStatus/State` を読む関数だけ。

mod client;
mod error;

pub use client::{ConnectedClient, OpcUaClient};
pub use error::Error;

/// 読み取り結果を解釈するときの共有語彙。
pub mod model {
    #[doc(inline)]
    pub use banto_opcua_core::{NodeId, Quality, StatusCode};
}
