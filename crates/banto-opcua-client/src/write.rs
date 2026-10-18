//! まとめて Write の結末（ADR-0005）。
//!
//! 分けて送った Write が途中で失敗すると、先に送った塊は機器に届いている。
//! そこで失敗にはノードごとの結末を持たせ、書けたノード・書けたか分からない
//! ノード・書いていないノードを呼び出し側が区別できるようにする。

use banto_opcua_core::StatusCode;
use opcua::{
    core::ResponseMessage,
    types::{Error as UaError, StatusCode as UaStatusCode},
};

use crate::{
    Error,
    chunk::{ChunkError, ChunkFailure},
    convert::core_status_code,
};

/// 1 ノードぶんの Write の結末。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum WriteOutcome {
    /// ノード単位の結果が決まった。サーバーが返した StatusCode（Good でも Bad でも）か、
    /// 送らずに決めた結果（`nsu=` を解決できなかったノードの `BadNodeIdUnknown`）。
    Completed(StatusCode),
    /// 送ったが結果を受け取れなかった。機器に届いたかもしれない。失敗の StatusCode を持つ。
    Unknown(StatusCode),
    /// サーバーが要求ごと拒否した。適用されていないことが確定している。拒否の StatusCode を持つ。
    Rejected(StatusCode),
    /// 送っていない。書かれていないことが確定している。
    NotSent,
}

impl WriteOutcome {
    /// 書かれていないことが確定しているか（[`Self::Rejected`] と [`Self::NotSent`]）。
    pub const fn is_definitely_not_written(self) -> bool {
        matches!(self, Self::Rejected(_) | Self::NotSent)
    }
}

/// まとめて Write の失敗。接続ごとの失敗で、ノードごとの結末を要求の順に持つ。
#[derive(Debug, thiserror::Error)]
#[error("{error}")]
#[non_exhaustive]
pub struct WriteFailure {
    /// 失敗の理由。
    #[source]
    pub error: Error,
    /// ノードごとの結末。要求と同じ数、同じ順。
    pub outcomes: Vec<WriteOutcome>,
}

impl WriteFailure {
    /// どのノードも送っていない失敗。
    pub(crate) fn nothing_sent(error: Error, len: usize) -> Self {
        Self {
            error,
            outcomes: vec![WriteOutcome::NotSent; len],
        }
    }
}

/// 1 つの塊の Write が失敗した理由。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChunkWriteFailure {
    /// 送っていない（未接続、または張り直していた）。
    NotSent,
    /// サーバーが要求ごと拒否した（サービスの結果が Bad、または ServiceFault）。
    Rejected(StatusCode),
    /// 送ったが結果が分からない。
    Unknown(StatusCode),
}

impl ChunkWriteFailure {
    /// 塊を小さくすれば通る拒否。
    pub(crate) fn is_too_many_operations(&self) -> bool {
        *self == Self::Rejected(StatusCode::BAD_TOO_MANY_OPERATIONS)
    }

    fn outcome(self) -> WriteOutcome {
        match self {
            Self::NotSent => WriteOutcome::NotSent,
            Self::Rejected(status) => WriteOutcome::Rejected(status),
            Self::Unknown(status) => WriteOutcome::Unknown(status),
        }
    }
}

/// `AsyncSecureChannel::send` の結果を、塊のノードごとの StatusCode か失敗に分ける。
///
/// - 送る前の失敗は `BadNotConnected` だけ（転送路が無いときに async-opcua が返す）。
///   ほかの送信の失敗は、要求を送った後かもしれないので結果不明にする。
/// - Write の応答のサービスの結果が Bad、または結果が Bad の ServiceFault は、要求ごとの拒否。
/// - 結果の数が塊と合わない応答と、想定外の応答は結果不明（受け取ったが対応が取れない）。
pub(crate) fn classify_response(
    response: Result<ResponseMessage, UaError>,
    chunk_len: usize,
) -> Result<Vec<UaStatusCode>, ChunkWriteFailure> {
    match response {
        Err(error) if error.status() == UaStatusCode::BadNotConnected => {
            Err(ChunkWriteFailure::NotSent)
        }
        Err(error) => Err(ChunkWriteFailure::Unknown(core_status_code(error.status()))),
        Ok(ResponseMessage::Write(response)) => {
            let service_result = response.response_header.service_result;
            if service_result.is_bad() {
                return Err(ChunkWriteFailure::Rejected(core_status_code(
                    service_result,
                )));
            }
            match response.results {
                Some(results) if results.len() == chunk_len => Ok(results),
                _ => Err(ChunkWriteFailure::Unknown(StatusCode::BAD_UNKNOWN_RESPONSE)),
            }
        }
        Ok(ResponseMessage::ServiceFault(fault))
            if fault.response_header.service_result.is_bad() =>
        {
            Err(ChunkWriteFailure::Rejected(core_status_code(
                fault.response_header.service_result,
            )))
        }
        Ok(_) => Err(ChunkWriteFailure::Unknown(StatusCode::BAD_UNKNOWN_RESPONSE)),
    }
}

/// 分けて送った Write の失敗を、送ったノードの並びの結末にする。
///
/// 失敗より前の塊は [`WriteOutcome::Completed`]、失敗した塊はその理由、それ以降は
/// [`WriteOutcome::NotSent`]。
pub(crate) fn outcomes_of(
    error: ChunkError<UaStatusCode, ChunkWriteFailure>,
    sent_len: usize,
) -> (Vec<WriteOutcome>, ChunkWriteFailure) {
    let mut outcomes: Vec<_> = error
        .done
        .into_iter()
        .map(|status| WriteOutcome::Completed(core_status_code(status)))
        .collect();
    let failure = match error.failure {
        ChunkFailure::Request { error, chunk_len } => {
            outcomes.extend(std::iter::repeat_n(error.outcome(), chunk_len));
            error
        }
        // classify_response が数を確かめるので、ここには来ない。来たら結果不明にする。
        ChunkFailure::CountMismatch { expected, .. } => {
            let failure = ChunkWriteFailure::Unknown(StatusCode::BAD_UNKNOWN_RESPONSE);
            outcomes.extend(std::iter::repeat_n(failure.outcome(), expected));
            failure
        }
    };
    outcomes.resize(sent_len, WriteOutcome::NotSent);
    (outcomes, failure)
}

/// 失敗した塊の理由を、[`WriteFailure::error`] にする。
pub(crate) fn error_of(failure: ChunkWriteFailure) -> Error {
    match failure {
        ChunkWriteFailure::NotSent => Error::Disconnected,
        ChunkWriteFailure::Rejected(status) => {
            Error::Write(format!("サーバーが Write の要求を拒否しました: {status}"))
        }
        ChunkWriteFailure::Unknown(status) => Error::Write(format!(
            "Write の結果を受け取れませんでした（届いたかもしれません）: {status}"
        )),
    }
}

#[cfg(test)]
mod tests {
    use opcua::types::{ResponseHeader, ServiceFault, WriteResponse};

    use super::*;

    fn write_response(
        service_result: UaStatusCode,
        results: Option<Vec<UaStatusCode>>,
    ) -> ResponseMessage {
        ResponseMessage::Write(Box::new(WriteResponse {
            response_header: ResponseHeader {
                service_result,
                ..Default::default()
            },
            results,
            diagnostic_infos: None,
        }))
    }

    #[test]
    fn per_node_results_are_returned_when_counts_match() {
        let results = vec![UaStatusCode::Good, UaStatusCode::BadNotWritable];
        assert_eq!(
            classify_response(
                Ok(write_response(UaStatusCode::Good, Some(results.clone()))),
                2
            ),
            Ok(results)
        );
    }

    #[test]
    fn not_connected_is_the_only_send_error_that_proves_nothing_was_sent() {
        assert_eq!(
            classify_response(Err(UaError::new(UaStatusCode::BadNotConnected, "")), 2),
            Err(ChunkWriteFailure::NotSent)
        );
        for status in [
            UaStatusCode::BadTimeout,
            UaStatusCode::BadConnectionClosed,
            UaStatusCode::BadCommunicationError,
            UaStatusCode::BadEncodingLimitsExceeded,
        ] {
            assert_eq!(
                classify_response(Err(UaError::new(status, "")), 2),
                Err(ChunkWriteFailure::Unknown(core_status_code(status))),
                "{status}"
            );
        }
    }

    #[test]
    fn bad_service_result_and_service_fault_are_rejections() {
        assert_eq!(
            classify_response(
                Ok(write_response(UaStatusCode::BadTooManyOperations, None)),
                2
            ),
            Err(ChunkWriteFailure::Rejected(
                StatusCode::BAD_TOO_MANY_OPERATIONS
            ))
        );
        let fault = ResponseMessage::ServiceFault(Box::new(ServiceFault {
            response_header: ResponseHeader {
                service_result: UaStatusCode::BadSessionIdInvalid,
                ..Default::default()
            },
        }));
        assert_eq!(
            classify_response(Ok(fault), 2),
            Err(ChunkWriteFailure::Rejected(core_status_code(
                UaStatusCode::BadSessionIdInvalid
            )))
        );
    }

    #[test]
    fn mismatched_or_unexpected_responses_are_unknown() {
        let unknown = Err(ChunkWriteFailure::Unknown(StatusCode::BAD_UNKNOWN_RESPONSE));
        assert_eq!(
            classify_response(
                Ok(write_response(
                    UaStatusCode::Good,
                    Some(vec![UaStatusCode::Good])
                )),
                2
            ),
            unknown
        );
        assert_eq!(
            classify_response(Ok(write_response(UaStatusCode::Good, None)), 2),
            unknown
        );
        assert_eq!(
            classify_response(Ok(ResponseMessage::ServiceFault(Box::default())), 2),
            unknown,
            "結果が Good の ServiceFault"
        );
        assert_eq!(
            classify_response(Ok(ResponseMessage::Read(Box::default())), 2),
            unknown,
            "Write 以外の応答"
        );
    }

    #[test]
    fn outcomes_mark_done_failing_and_unsent_chunks() {
        let (outcomes, failure) = outcomes_of(
            ChunkError {
                done: vec![UaStatusCode::Good, UaStatusCode::BadNotWritable],
                failure: ChunkFailure::Request {
                    error: ChunkWriteFailure::Unknown(StatusCode::BAD_COMMUNICATION_ERROR),
                    chunk_len: 2,
                },
            },
            6,
        );
        assert_eq!(
            outcomes,
            vec![
                WriteOutcome::Completed(StatusCode::GOOD),
                WriteOutcome::Completed(StatusCode::BAD_NOT_WRITABLE),
                WriteOutcome::Unknown(StatusCode::BAD_COMMUNICATION_ERROR),
                WriteOutcome::Unknown(StatusCode::BAD_COMMUNICATION_ERROR),
                WriteOutcome::NotSent,
                WriteOutcome::NotSent,
            ]
        );
        assert_eq!(
            failure,
            ChunkWriteFailure::Unknown(StatusCode::BAD_COMMUNICATION_ERROR)
        );
        assert!(!outcomes[2].is_definitely_not_written());
        assert!(outcomes[4].is_definitely_not_written());
    }

    #[test]
    fn rejected_chunk_is_definitely_not_written() {
        let (outcomes, _) = outcomes_of(
            ChunkError {
                done: vec![],
                failure: ChunkFailure::Request {
                    error: ChunkWriteFailure::Rejected(StatusCode::BAD_TOO_MANY_OPERATIONS),
                    chunk_len: 1,
                },
            },
            2,
        );
        assert_eq!(
            outcomes,
            vec![
                WriteOutcome::Rejected(StatusCode::BAD_TOO_MANY_OPERATIONS),
                WriteOutcome::NotSent,
            ]
        );
        assert!(
            outcomes
                .iter()
                .all(|outcome| outcome.is_definitely_not_written())
        );
    }
}
