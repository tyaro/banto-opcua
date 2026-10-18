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
//! Phase 1 の固定タグ `Speed` / `Running` に加え、タグの追加・削除と
//! 同じ NodeId の復帰、実タグの品質・時刻付き更新を扱う。書き込みは
//! [`OpcUaServer::start_with_write_handler`] で渡すハンドラーを通してだけ
//! 受け付ける（ADR-0004）。ハンドラーを渡さなければ読み取り専用。
//! プロセス内の疎通は `tests/fixed_tags.rs`、`tests/dynamic_nodes.rs`、
//! `tests/tag_readings.rs`。

mod config;
mod error;
mod fixed_tags;
mod reading;
mod server;
mod tag_manager;
mod write;

pub use config::ServerConfig;
pub use error::Error;
pub use reading::TagReading;
pub use server::{OpcUaServer, RunningServer};
pub use write::{WriteFuture, WriteHandler};

/// タグノードの語彙。プロトコル実装の型ではなく core の型を公開する。
pub mod model {
    #[doc(inline)]
    pub use banto_opcua_core::{
        AccessLevel, DataType, Identifier, MappedReading, MappedValue, NodeClass, NodeId,
        PublishDecision, Quality, SampleTimestamps, StatusCode, TagType, VariableNode, WriteClient,
        WritePrincipal, WriteRejection, WriteRequest, status_code_for_write_rejection,
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
        assert_eq!(Core::BAD_NOT_WRITABLE.code(), Ua::BadNotWritable.bits());
        assert_eq!(
            Core::BAD_USER_ACCESS_DENIED.code(),
            Ua::BadUserAccessDenied.bits()
        );
        assert_eq!(Core::BAD_TYPE_MISMATCH.code(), Ua::BadTypeMismatch.bits());
        assert_eq!(Core::BAD_OUT_OF_RANGE.code(), Ua::BadOutOfRange.bits());
        assert_eq!(Core::BAD_OUT_OF_SERVICE.code(), Ua::BadOutOfService.bits());
        assert_eq!(
            Core::BAD_REQUEST_NOT_ALLOWED.code(),
            Ua::BadRequestNotAllowed.bits()
        );
        assert_eq!(
            Core::BAD_COMMUNICATION_ERROR.code(),
            Ua::BadCommunicationError.bits()
        );
        assert_eq!(Core::BAD_INTERNAL_ERROR.code(), Ua::BadInternalError.bits());
        assert_eq!(Core::BAD_NOT_SUPPORTED.code(), Ua::BadNotSupported.bits());
        assert_eq!(
            Core::BAD_TOO_MANY_OPERATIONS.code(),
            Ua::BadTooManyOperations.bits()
        );
    }
}
