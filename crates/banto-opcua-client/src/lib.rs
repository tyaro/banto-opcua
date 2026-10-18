//! banto 向け OPC UA クライアントの部品（ADR-0005）。
//!
//! Hub のドライバが外部の OPC UA サーバーを周期 Read・Write するための部品。
//! SecurityPolicy `None` / Anonymous で接続し、ノードの並びをまとめて読み書きする。
//! 周期と再接続は呼び出し側（Hub の broker）の役目で、既定では自動再接続を止める。
//! サーバークレートのプロセス内テストもこのクレートを使う。
//! 公開 API に async-opcua の型は出さない（ADR-0001）。

mod chunk;
mod client;
mod config;
mod convert;
mod error;
mod write;

pub use client::{ConnectedClient, OpcUaClient};
pub use config::{ClientConfig, ReconnectPolicy};
pub use convert::NodeReading;
pub use error::Error;
pub use write::{WriteFailure, WriteOutcome};

/// 読み取り結果を解釈するときの共有語彙。
pub mod model {
    #[doc(inline)]
    pub use banto_opcua_core::{
        NamespaceRef, NodeAddress, NodeAddressError, NodeId, Quality, ScalarType, ScalarValue,
        StatusCode,
    };
}
