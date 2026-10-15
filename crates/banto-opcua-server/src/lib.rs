//! banto 向け OPC UA サーバーの足場。
//!
//! 既定の待受は `127.0.0.1`。セキュリティは Phase 1 の試験用として
//! SecurityPolicy `None` / MessageSecurityMode `None` / Anonymous だけを
//! 開く。証明書・ユーザー認証は Phase 3（tyaro/banto-industrial#201）。
//! ループバック以外へバインドすると、そのインターフェースに対して匿名かつ
//! 平文で待受する。LAN 公開は呼び出し側の明示的な opt-in であり、この
//! クレートの既定ではない。
//!
//! タグのアドレス空間はまだ作らない。起動すると async-opcua の標準
//! 名前空間だけが載る。プロセス内の疎通は `tests/in_process.rs`。

mod config;
mod error;
mod server;

pub use config::{ServerConfig, endpoint_url};
pub use error::Error;
pub use server::{OpcUaServer, RunningServer};

/// タグノードの語彙。プロトコル実装の型ではなく core の型を公開する。
pub mod model {
    #[doc(inline)]
    pub use banto_opcua_core::{
        AccessLevel, DataType, Identifier, NodeClass, NodeId, PublishDecision, Quality, StatusCode,
        TagType, VariableNode,
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
