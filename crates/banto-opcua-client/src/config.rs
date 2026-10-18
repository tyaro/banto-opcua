//! 接続の設定（ADR-0005）。SecurityPolicy と認証の項目は Phase 3 まで置かない。

use std::{
    num::{NonZeroU64, NonZeroUsize},
    time::Duration,
};

/// 接続が切れたときに、誰が張り直すか。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ReconnectPolicy {
    /// 既定。張り直さない（ADR-0005）。切れた後の呼び出しはすべて
    /// [`crate::Error::Disconnected`] を返し、呼び出し側（Hub の broker）が新しく接続する。
    Disabled,
    /// async-opcua の自動再接続で、この部品が張り直す。
    ///
    /// banto の broker の仕様が変わったときのために残す選択肢。張り直している間の
    /// 呼び出しは [`crate::Error::Disconnected`] を返す。張り直した後は
    /// NamespaceArray と OperationLimits を読み直す。この設定を使うときは、
    /// 呼び出し側は同じ接続を張り直さない（二重になる）。
    ///
    /// 切れたらすぐに 1 回張り直し、失敗するたびに待ち時間を倍にする。
    /// `max_retries` を使い切ると諦め、以降の呼び出しは [`ReconnectPolicy::Disabled`] と
    /// 同じく、すべて [`crate::Error::Disconnected`] を返す。
    Automatic {
        /// 張り直しに 1 回失敗した後の、最初の待ち時間。
        initial_delay: Duration,
        /// 待ち時間を倍にしていくときの上限。
        max_delay: Duration,
        /// 最初の張り直しの後に、続けて失敗してよい回数（`Some(n)` は合わせて n + 1 回
        /// 試す）。`None` は諦めない。
        max_retries: Option<u32>,
    },
}

/// 接続先と時間切れ、要求の分割の設定。
///
/// 外部クレートでは [`ClientConfig::new`] と `with_*` メソッドで生成する。
/// 新しい設定項目が増えても構造体リテラルを更新せずに済む。
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ClientConfig {
    /// `opc.tcp://` のエンドポイント URL。
    pub endpoint_url: String,
    /// エンドポイントを選んだ後、セッションの確立を待つ時間。既定は 20 秒。
    ///
    /// エンドポイントの取得（GetEndpoints）には [`Self::request_timeout`] が効く。
    pub connect_timeout: Duration,
    /// 1 回の要求の応答を待つ時間。既定は 10 秒。収集の周期より短くする。
    pub request_timeout: Duration,
    /// keep-alive の Read を送る間隔。既定は 5 秒。
    ///
    /// セッションの時間切れ（async-opcua の既定で 60 秒を要求する）より短くする。
    pub keep_alive_interval: Duration,
    /// keep-alive が続けて失敗したら接続を閉じる回数。既定は 3。
    ///
    /// TCP が閉じないまま応答が止まる切断を、これで見つける。keep-alive の Read も
    /// [`Self::request_timeout`] まで応答を待つので、見つけるまでの時間はおよそ
    /// `回数 × max(keep_alive_interval, request_timeout) + request_timeout`
    /// （既定で 40 秒前後）。
    pub max_failed_keep_alive_count: NonZeroU64,
    /// サーバーが `MaxNodesPerRead` を広告しない（0、ノードが無い、Bad）ときに、
    /// 1 回の Read へ入れるノード数。既定は 100。
    pub default_max_nodes_per_read: NonZeroUsize,
    /// 接続が切れたときに張り直すか。既定は [`ReconnectPolicy::Disabled`]。
    pub reconnect: ReconnectPolicy,
}

const DEFAULT_MAX_NODES_PER_READ: NonZeroUsize = NonZeroUsize::new(100).unwrap();
const DEFAULT_MAX_FAILED_KEEP_ALIVE_COUNT: NonZeroU64 = NonZeroU64::new(3).unwrap();

impl ClientConfig {
    /// 既定の時間切れで `endpoint_url` へ接続する設定を作る。
    pub fn new(endpoint_url: impl Into<String>) -> Self {
        Self {
            endpoint_url: endpoint_url.into(),
            connect_timeout: Duration::from_secs(20),
            request_timeout: Duration::from_secs(10),
            keep_alive_interval: Duration::from_secs(5),
            max_failed_keep_alive_count: DEFAULT_MAX_FAILED_KEEP_ALIVE_COUNT,
            default_max_nodes_per_read: DEFAULT_MAX_NODES_PER_READ,
            reconnect: ReconnectPolicy::Disabled,
        }
    }

    /// セッションの確立を待つ時間を設定する。
    pub fn with_connect_timeout(mut self, timeout: Duration) -> Self {
        self.connect_timeout = timeout;
        self
    }

    /// 1 回の要求の応答を待つ時間を設定する。
    pub fn with_request_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = timeout;
        self
    }

    /// keep-alive の間隔を設定する。
    pub fn with_keep_alive_interval(mut self, interval: Duration) -> Self {
        self.keep_alive_interval = interval;
        self
    }

    /// keep-alive が続けて失敗したら接続を閉じる回数を設定する。
    pub fn with_max_failed_keep_alive_count(mut self, count: NonZeroU64) -> Self {
        self.max_failed_keep_alive_count = count;
        self
    }

    /// サーバーが上限を広告しないときの、1 回の Read のノード数を設定する。
    pub fn with_default_max_nodes_per_read(mut self, max: NonZeroUsize) -> Self {
        self.default_max_nodes_per_read = max;
        self
    }

    /// 接続が切れたときに張り直すかを設定する。
    pub fn with_reconnect(mut self, reconnect: ReconnectPolicy) -> Self {
        self.reconnect = reconnect;
        self
    }

    /// 0 の時間を拒否する。async-opcua では 0 が「待たない」や「送らない」になる。
    pub(crate) fn validate(&self) -> Result<(), String> {
        let mut durations = vec![
            ("connect_timeout", self.connect_timeout),
            ("request_timeout", self.request_timeout),
            ("keep_alive_interval", self.keep_alive_interval),
        ];
        if let ReconnectPolicy::Automatic {
            initial_delay,
            max_delay,
            ..
        } = self.reconnect
        {
            if max_delay < initial_delay {
                return Err("max_delay は initial_delay 以上にしてください".to_owned());
            }
            durations.extend([("initial_delay", initial_delay), ("max_delay", max_delay)]);
        }
        for (name, duration) in durations {
            if duration.is_zero() {
                return Err(format!("{name} に 0 は指定できません"));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_detect_silent_loss_and_validate() {
        let config = ClientConfig::new("opc.tcp://127.0.0.1:4840/");
        assert_eq!(config.max_failed_keep_alive_count.get(), 3);
        assert!(config.keep_alive_interval < Duration::from_secs(60));
        assert_eq!(config.validate(), Ok(()));
        assert_eq!(config.reconnect, ReconnectPolicy::Disabled);
    }

    #[test]
    fn automatic_reconnect_needs_nonzero_increasing_delays() {
        let config = ClientConfig::new("opc.tcp://127.0.0.1:4840/");
        let automatic = |initial_delay, max_delay| {
            config.clone().with_reconnect(ReconnectPolicy::Automatic {
                initial_delay,
                max_delay,
                max_retries: None,
            })
        };
        assert_eq!(
            automatic(Duration::from_secs(1), Duration::from_secs(30)).validate(),
            Ok(())
        );
        assert!(
            automatic(Duration::ZERO, Duration::from_secs(30))
                .validate()
                .is_err()
        );
        assert!(
            automatic(Duration::from_secs(2), Duration::from_secs(1))
                .validate()
                .is_err()
        );
    }

    #[test]
    fn zero_durations_are_rejected() {
        let config = ClientConfig::new("opc.tcp://127.0.0.1:4840/");
        for invalid in [
            config.clone().with_connect_timeout(Duration::ZERO),
            config.clone().with_request_timeout(Duration::ZERO),
            config.clone().with_keep_alive_interval(Duration::ZERO),
        ] {
            assert!(invalid.validate().is_err());
        }
    }
}
