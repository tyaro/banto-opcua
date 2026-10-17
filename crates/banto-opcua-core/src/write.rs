//! OPC UA の Write を利用者の書き込みハンドラーへ渡す語彙（ADR-0004）。
//!
//! サーバーは検査を通った Write だけを [`WriteRequest`] にしてハンドラーへ渡し、
//! ハンドラーは成功か [`WriteRejection`] を返す。拒否の理由から StatusCode への
//! 写像は [`status_code_for_write_rejection`]。

use crate::{MappedValue, StatusCode};

/// 書いた主体。今は匿名だけで、UserName と証明書の主体は Phase 3 で足す。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum WritePrincipal {
    /// Anonymous のユーザートークン。匿名を許すかどうかはハンドラーが決める。
    Anonymous,
}

/// 接続元の情報。async-opcua のセッションから取れるものに限る。
///
/// 接続元の IP アドレスはセッションに伝わっていないので含まない。値は
/// クライアントの自己申告で、主体の確認には使えない。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct WriteClient {
    /// セッションを作ったエンドポイント URL。
    pub endpoint_url: String,
    /// サーバーが割り当てたセッション ID（NodeId の文字列表現）。
    pub session_id: String,
    /// クライアントが付けたセッション名。
    pub session_name: String,
    /// クライアントの ApplicationUri。
    pub application_uri: String,
    /// クライアントの ApplicationName。
    pub application_name: String,
}

impl WriteClient {
    pub fn new(
        endpoint_url: impl Into<String>,
        session_id: impl Into<String>,
        session_name: impl Into<String>,
        application_uri: impl Into<String>,
        application_name: impl Into<String>,
    ) -> Self {
        Self {
            endpoint_url: endpoint_url.into(),
            session_id: session_id.into(),
            session_name: session_name.into(),
            application_uri: application_uri.into(),
            application_name: application_name.into(),
        }
    }
}

/// サーバーの検査を通った 1 ノードぶんの書き込み。
///
/// 値の型はノードの DataType と一致し、Double は有限値に限る。書き込みの
/// SourceTimestamp と ServerTimestamp は捨ててあり、ここには含まない。
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct WriteRequest {
    /// 対象タグの接続名。
    pub connection: String,
    /// 対象タグのグループ名。
    pub group: String,
    /// 対象タグのタグ名。
    pub tag: String,
    /// 書く値。
    pub value: MappedValue,
    /// 書いた主体。
    pub principal: WritePrincipal,
    /// 接続元の情報。
    pub client: WriteClient,
}

impl WriteRequest {
    pub fn new(
        connection: impl Into<String>,
        group: impl Into<String>,
        tag: impl Into<String>,
        value: MappedValue,
        principal: WritePrincipal,
        client: WriteClient,
    ) -> Self {
        Self {
            connection: connection.into(),
            group: group.into(),
            tag: tag.into(),
            value,
            principal,
            client,
        }
    }
}

/// ハンドラーが書き込みを拒否した理由。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum WriteRejection {
    /// タグが無い。
    TagNotFound,
    /// 書けないタグ。
    NotWritable,
    /// 主体に権限が無い。
    AccessDenied,
    /// 値の型がタグに合わない。
    TypeMismatch,
    /// 値が範囲外（整数のタグへの小数を含む）。
    OutOfRange,
    /// 書き込みを受け付けていない（全体の停止、収集の停止、無効なタグ）。
    OutOfService,
    /// 回数制限を超えた。
    RateLimited,
    /// 機器との通信に失敗した。
    CommunicationError,
    /// 内部エラー（監査の失敗を含む）。
    Internal,
}

/// 拒否の理由を Write の結果の StatusCode へ写す（ADR-0004 の表）。
///
/// 回数制限に `BadTooManyOperations` は使わない。Part 4 ではサービス全体の
/// 結果で、クライアントが要求を分けて再送してしまう。
pub const fn status_code_for_write_rejection(rejection: WriteRejection) -> StatusCode {
    match rejection {
        WriteRejection::TagNotFound => StatusCode::BAD_NODE_ID_UNKNOWN,
        WriteRejection::NotWritable => StatusCode::BAD_NOT_WRITABLE,
        WriteRejection::AccessDenied => StatusCode::BAD_USER_ACCESS_DENIED,
        WriteRejection::TypeMismatch => StatusCode::BAD_TYPE_MISMATCH,
        WriteRejection::OutOfRange => StatusCode::BAD_OUT_OF_RANGE,
        WriteRejection::OutOfService => StatusCode::BAD_OUT_OF_SERVICE,
        WriteRejection::RateLimited => StatusCode::BAD_REQUEST_NOT_ALLOWED,
        WriteRejection::CommunicationError => StatusCode::BAD_COMMUNICATION_ERROR,
        WriteRejection::Internal => StatusCode::BAD_INTERNAL_ERROR,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_rejection_is_bad_and_distinct() {
        let all = [
            WriteRejection::TagNotFound,
            WriteRejection::NotWritable,
            WriteRejection::AccessDenied,
            WriteRejection::TypeMismatch,
            WriteRejection::OutOfRange,
            WriteRejection::OutOfService,
            WriteRejection::RateLimited,
            WriteRejection::CommunicationError,
            WriteRejection::Internal,
        ];
        let codes: std::collections::HashSet<_> = all
            .iter()
            .map(|&rejection| status_code_for_write_rejection(rejection))
            .collect();
        assert_eq!(codes.len(), all.len());
        assert!(codes.iter().all(|code| code.is_bad()));
    }
}
