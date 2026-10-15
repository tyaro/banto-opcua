//! OPC UA DateTime と [`std::time::SystemTime`] の変換。
//!
//! OPC UA DateTime は 1601-01-01 UTC からの 100 ナノ秒刻み（符号付き 64 ビット）。
//! `chrono` は引かない（banto ADR-0002 の自前日付変換と同じ理由）。
//!
//! SourceTimestamp は呼び出し側が渡した採取時刻を使う。Read のたびに現在時刻へ
//! 更新する関数は置かない（#201 レビュー: 古い値の時刻を今にしない）。
//! banto-hub の `ptime_ms` が「最後の試行時刻」であり PLC の時刻ではない点は、
//! ハブ側の契約として未決。このクレートはその時刻を作らない。

use std::time::{SystemTime, UNIX_EPOCH};

/// Unix エポックを OPC UA の 100 ナノ秒刻みで表した値。
pub const UNIX_EPOCH_TICKS: i64 = 11_644_473_600 * 10_000_000;

/// 採取時刻と、サーバーが DataValue を組み立てた時刻。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SampleTimestamps {
    /// 値を採取した時刻。未取得なら `None`。
    pub source: Option<SystemTime>,
    /// サーバーがこの値を公開用に組み立てた時刻。
    pub server: SystemTime,
}

impl SampleTimestamps {
    /// 採取時刻とサーバー時刻から作る。採取時刻を今で埋めない。
    pub const fn new(source: Option<SystemTime>, server: SystemTime) -> Self {
        Self { source, server }
    }

    /// SourceTimestamp の OPC UA 刻み。未取得、または範囲外なら `None`。
    pub fn source_ticks(self) -> Option<i64> {
        self.source.and_then(opcua_ticks_from_system_time)
    }

    /// ServerTimestamp の OPC UA 刻み。範囲外なら `None`。
    pub fn server_ticks(self) -> Option<i64> {
        opcua_ticks_from_system_time(self.server)
    }
}

/// `SystemTime` を OPC UA DateTime の刻みへ変換する。
///
/// Unix エポックより前、または i64 に収まらない時刻は `None`。
pub fn opcua_ticks_from_system_time(time: SystemTime) -> Option<i64> {
    let since_unix = time.duration_since(UNIX_EPOCH).ok()?;
    let nanos = i64::try_from(since_unix.as_nanos()).ok()?;
    let ticks = nanos / 100;
    ticks.checked_add(UNIX_EPOCH_TICKS)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn unix_epoch_is_the_documented_offset() {
        assert_eq!(
            opcua_ticks_from_system_time(UNIX_EPOCH),
            Some(UNIX_EPOCH_TICKS)
        );
    }

    #[test]
    fn one_second_after_the_epoch_is_ten_million_ticks() {
        let time = UNIX_EPOCH + Duration::from_secs(1);
        assert_eq!(
            opcua_ticks_from_system_time(time),
            Some(UNIX_EPOCH_TICKS + 10_000_000)
        );
    }

    #[test]
    fn missing_source_timestamp_stays_absent() {
        let sample = SampleTimestamps::new(None, UNIX_EPOCH);
        assert_eq!(sample.source_ticks(), None);
        assert_eq!(sample.server_ticks(), Some(UNIX_EPOCH_TICKS));
    }
}
