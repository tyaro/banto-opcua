//! banto 向け OPC UA クライアントの部品（ADR-0005）。
//!
//! Hub のドライバが外部の OPC UA サーバーを周期 Read・Write するための部品。
//! ノードの並びをまとめて読み書きする。既定は SecurityPolicy `None` / Anonymous で、
//! 署名・暗号化と UserName、サーバーの証明書の信頼の一覧で接続できる（ADR-0009）。
//! 周期と再接続は呼び出し側（Hub の broker）の役目で、既定では自動再接続を止める。
//! サーバークレートのプロセス内テストもこのクレートを使う。
//! 公開 API に async-opcua の型は出さない（ADR-0001）。

mod chunk;
mod client;
mod config;
mod convert;
mod error;
mod metadata;
mod security;
mod subscribe;
mod write;

pub use client::{ConnectedClient, OpcUaClient};
pub use config::{ClientConfig, ClientIdentity, ReconnectPolicy, SubscriptionSettings};
pub use convert::NodeReading;
pub use error::{CertificateProblem, Error};
pub use metadata::NodeMetadata;
pub use security::EndpointInfo;

/// サーバーの証明書の信頼の一覧（ADR-0009）。サーバーのクレートと同じ型。
pub use banto_opcua_pki::{TrustList, TrustState, TrustedCertificate};
pub use write::{WriteFailure, WriteOutcome};

/// 読み取り結果を解釈するときの共有語彙。
pub mod model {
    #[doc(inline)]
    pub use banto_opcua_core::{
        CertificateInfo, EngineeringUnits, EuRange, MessageSecurityMode, NamespaceRef, NodeAddress,
        NodeAddressError, NodeId, Password, Quality, ScalarType, ScalarValue, SecurityPolicy,
        StatusCode, TagMetadata, TwoStateLabels, UserTokenType,
    };
}
