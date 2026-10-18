//! banto ファミリー向け OPC UA の共有語彙。
//!
//! このクレートはプロトコルを話さない。値の型、品質から StatusCode への写像、
//! 時刻から OPC UA DateTime への変換、ノードの形、書き込みハンドラーとの
//! 受け渡しの型、クライアントが扱う NodeId の文字列表現と値の型だけを持つ。サーバー（`banto-opcua-server`）とクライアント
//! （`banto-opcua-client`）が async-opcua との境界でこれらの型を使う。依存をここに置かないのは、写像を
//! 単体テストできるようにすることと、MPL-2.0 の型を MIT の公開 API に
//! 漏らさないため（ADR-0001）。
//!
//! 写像の方針はリポジトリ README の「決定事項」（2026-10-15）。削除ノードの
//! 購読と品質だけの変化通知は server で扱う。非有限値の判定は core に置く。

mod address;
mod node;
mod quality;
mod scalar;
mod time;
mod value;
mod write;

pub use address::{NamespaceRef, NodeAddress, NodeAddressError, UnsupportedIdentifierType};
pub use node::{AccessLevel, Identifier, NodeClass, NodeId, VariableNode};
pub use quality::{Quality, StatusCode, status_code_for_quality};
pub use scalar::{ScalarType, ScalarValue};
pub use time::{
    SampleTimestamps, UNIX_EPOCH_TICKS, opcua_ticks_from_system_time, system_time_from_opcua_ticks,
};
pub use value::{
    DataType, MappedReading, MappedValue, PublishDecision, TagType, map_current_value,
    publish_decision,
};
pub use write::{
    WriteClient, WritePrincipal, WriteRejection, WriteRequest, status_code_for_write_rejection,
};
