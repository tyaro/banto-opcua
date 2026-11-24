//! 起動時に渡す、利用者の処理の組。

use std::sync::Arc;

use crate::{UserAuthenticator, WriteHandler};

/// [`crate::OpcUaServer::start_with_handlers`] に渡す、利用者の処理。どれも省ける。
#[derive(Clone, Default)]
#[non_exhaustive]
pub struct ServerHandlers {
    /// 書き込みハンドラー（ADR-0004）。無ければすべてのタグは読み取り専用。
    pub write_handler: Option<Arc<dyn WriteHandler>>,
    /// 利用者名とパスワードの照合（ADR-0009）。無ければ UserName を受け付けない。
    pub user_authenticator: Option<Arc<dyn UserAuthenticator>>,
}

impl ServerHandlers {
    /// 書き込みハンドラーを設定する。
    pub fn with_write_handler(mut self, handler: Arc<dyn WriteHandler>) -> Self {
        self.write_handler = Some(handler);
        self
    }

    /// 利用者の照合を設定する。
    pub fn with_user_authenticator(mut self, authenticator: Arc<dyn UserAuthenticator>) -> Self {
        self.user_authenticator = Some(authenticator);
        self
    }
}

impl std::fmt::Debug for ServerHandlers {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServerHandlers")
            .field("write_handler", &self.write_handler.is_some())
            .field("user_authenticator", &self.user_authenticator.is_some())
            .finish()
    }
}
