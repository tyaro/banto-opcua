//! banto ファミリー向け OPC UA の共有語彙。
//!
//! このクレートはプロトコルを話さない。値の型、品質から StatusCode への写像、
//! 時刻から OPC UA DateTime への変換、ノードの形だけを持つ。サーバー
//! （`banto-opcua-server`）とクライアント（`banto-opcua-client`）が
//! async-opcua との境界でこれらの型を使う。依存をここに置かないのは、写像を
//! 単体テストできるようにすることと、MPL-2.0 の型を MIT の公開 API に
//! 漏らさないため（ADR-0001）。
//!
//! 写像の方針はリポジトリ README の「決定事項」（2026-10-15）。削除ノードの
//! 購読、非有限値、品質だけの変化通知は Phase 2 で、ここには置かない。

mod node;
mod quality;
mod time;
mod value;

pub use node::{AccessLevel, Identifier, NodeClass, NodeId, VariableNode};
pub use quality::{Quality, StatusCode, status_code_for_quality};
pub use time::{SampleTimestamps, UNIX_EPOCH_TICKS, opcua_ticks_from_system_time};
pub use value::{DataType, PublishDecision, TagType, publish_decision};
