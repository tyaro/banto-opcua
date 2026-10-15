//! banto 向け OPC UA サーバーの足場。
//!
//! 既定の待受は `127.0.0.1`、既定ポートは 4840。セキュリティは Phase 1 の
//! 試験用として SecurityPolicy `None` / MessageSecurityMode `None` /
//! Anonymous だけを開く。証明書・ユーザー認証は Phase 3
//! （tyaro/banto-industrial#201）。
//!
//! 未指定アドレス（`0.0.0.0` / `::`）で待受するときは `advertised_host` が
//! 必須。ループバック以外への公開は明示したときだけで、警告ログを出す。
//!
//! Phase 1 の固定タグ `Speed` / `Running` を Browse / Read / Subscribe
//! できる。プロセス内の疎通は `tests/fixed_tags.rs`。

mod config;
mod error;
mod fixed_tags;
mod server;

pub use config::ServerConfig;
pub use error::Error;
pub use server::{OpcUaServer, RunningServer};

/// タグノードの語彙。プロトコル実装の型ではなく core の型を公開する。
pub mod model {
    #[doc(inline)]
    pub use banto_opcua_core::{
        AccessLevel, DataType, Identifier, MappedReading, MappedValue, NodeClass, NodeId,
        PublishDecision, Quality, StatusCode, TagType, VariableNode,
    };
}

#[cfg(test)]
mod tests {
    use banto_opcua_core::StatusCode as Core;
    use opcua::types::StatusCode as Ua;

    #[test]
    fn core_status_codes_match_async_opcua() {
        assert_eq!(Core::GOOD.code(), Ua::Good.bits());
        assert_eq!(Core::BAD.code(), Ua::Bad.bits());
        assert_eq!(
            Core::UNCERTAIN_LAST_USABLE_VALUE.code(),
            Ua::UncertainLastUsableValue.bits()
        );
        assert_eq!(
            Core::BAD_NODE_ID_UNKNOWN.code(),
            Ua::BadNodeIdUnknown.bits()
        );
    }
}
