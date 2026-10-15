//! banto ファミリー向け OPC UA の共有語彙。
//!
//! このクレートはプロトコルを話さない。値の型、品質から StatusCode への写像、
//! 時刻から OPC UA DateTime への変換、ノードの形だけを持つ。サーバー
//! （`banto-opcua-server`）とクライアント（`banto-opcua-client`）が
//! async-opcua との境界でこれらの型を使う。依存をここに置かないのは、写像を
//! 単体テストできるようにすることと、MPL-2.0 の型を MIT の公開 API に
//! 漏らさないため（ADR-0001）。
//!
//! 写像は tyaro/banto-industrial#201 の 2026-10-11 レビューが示した最小案に
//! 合わせた足場であり、オーナーの最終決定ではない。未決はリポジトリ README
//! の「オーナー判断待ち」を参照。

mod node;
mod quality;
mod time;
mod value;

pub use node::{AccessLevel, Identifier, NodeClass, NodeId, VariableNode};
pub use quality::{Quality, StatusCode, status_code_for_quality};
pub use time::{SampleTimestamps, UNIX_EPOCH_TICKS, opcua_ticks_from_system_time};
pub use value::{DataType, PublishDecision, TagType, publish_decision};
