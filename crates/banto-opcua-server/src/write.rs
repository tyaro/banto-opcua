//! 利用者が渡す書き込みハンドラー（ADR-0004）。

use std::{future::Future, pin::Pin};

use banto_opcua_core::{WriteRejection, WriteRequest};

/// [`WriteHandler::write`] が返す future。
pub type WriteFuture = Pin<Box<dyn Future<Output = Result<(), WriteRejection>> + Send + 'static>>;

/// OPC UA の Write を受け取り、機器へ書く利用者の処理。
///
/// サーバーは自分の検査（AccessLevel、型、StatusCode、IndexRange、同時実行数）を
/// 通ったノードだけを、1 件ずつ独立したタスクで渡す。返した future は
/// クライアントが切断しても最後まで走るので、監査は途中で切れない。
///
/// サーバー側では待つ上限を設けない。結果を返すまで Write の応答も返らないので、
/// 実装は有限の時間で結果を返すこと（機器との通信の時間切れなど）。成功を返しても
/// ノードの値は変わらない。機器から戻った値は
/// [`crate::RunningServer::update_tag_reading`] で反映する。
pub trait WriteHandler: Send + Sync + 'static {
    /// 1 ノードぶんの書き込みを実行する。
    fn write(&self, request: WriteRequest) -> WriteFuture;
}
