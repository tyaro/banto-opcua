//! OPC UA DateTime と [`std::time::SystemTime`] の変換。
//!
//! OPC UA DateTime は 1601-01-01 UTC からの 100 ナノ秒刻み（符号付き 64 ビット）。
//! `chrono` は引かない（banto ADR-0002 の自前日付変換と同じ理由）。
//!
//! 時刻の方針（2026-10-15 オーナー決定。実装は Phase 2）:
//! Good の SourceTimestamp は採取時刻、Bad / Stale は品質を認識した時刻、
//! 不明なら null。ハブが時刻を渡せるようになるまで、このクレートは渡された
//! [`SystemTime`] を変換するだけで、現在時刻では埋めない。

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
/// 先に 100 ナノ秒単位へ換算してから i64 へ入れる。ナノ秒のまま i64 へ
/// 入れると 2262 年付近で溢れる。Unix エポックより前、または 1601 年からの
/// 刻みが i64 に収まらない時刻は `None`。
pub fn opcua_ticks_from_system_time(time: SystemTime) -> Option<i64> {
    let since_unix = time.duration_since(UNIX_EPOCH).ok()?;
    opcua_ticks_from_unix_nanos(since_unix.as_nanos())
}

/// Unix エポックからのナノ秒を OPC UA の刻みへ変換する。
///
/// `SystemTime` の表現範囲（Windows は 30828 年付近で止まる）には依存しない。
fn opcua_ticks_from_unix_nanos(nanos_since_unix: u128) -> Option<i64> {
    let ticks_since_unix = nanos_since_unix / 100;
    let ticks_since_unix = i64::try_from(ticks_since_unix).ok()?;
    ticks_since_unix.checked_add(UNIX_EPOCH_TICKS)
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

    #[test]
    fn before_the_unix_epoch_is_none() {
        let time = UNIX_EPOCH
            .checked_sub(Duration::from_secs(1))
            .expect("unix epoch の 1 秒前");
        assert_eq!(opcua_ticks_from_system_time(time), None);
    }

    #[test]
    fn nanoseconds_past_i64_still_convert_when_ticks_fit() {
        // i64::MAX ナノ秒は 2262 年付近。ナノ秒を i64 に入れてから割ると None になる。
        let nanos = u128::from(i64::MAX as u64) + 1;
        let ticks_since_unix = i64::try_from(nanos / 100).expect("ticks");
        let expected = ticks_since_unix + UNIX_EPOCH_TICKS;
        assert_eq!(opcua_ticks_from_unix_nanos(nanos), Some(expected));
        let time = system_time_after_unix_nanos(nanos).expect("2262 年付近は SystemTime に収まる");
        assert_eq!(opcua_ticks_from_system_time(time), Some(expected));
    }

    #[test]
    fn year_2300_matches_the_opc_ua_tick_count() {
        // 2300-01-01 UTC。Windows の SystemTime にも収まる。
        let secs = 10_413_792_000_u64;
        let expected = 220_582_656_000_000_000_i64;
        assert_eq!(
            opcua_ticks_from_unix_nanos(u128::from(secs) * 1_000_000_000),
            Some(expected)
        );
        let time = UNIX_EPOCH
            .checked_add(Duration::from_secs(secs))
            .expect("2300 年は SystemTime に収まる");
        assert_eq!(opcua_ticks_from_system_time(time), Some(expected));
    }

    #[test]
    fn last_representable_tick_is_some_and_the_next_is_none() {
        // OPC UA の最終刻みは Windows の SystemTime では表せないので、ナノ秒の
        // 整数だけで検証する。
        let max_ticks_since_unix = u128::try_from(i64::MAX - UNIX_EPOCH_TICKS).expect("ticks");
        let nanos = max_ticks_since_unix * 100;
        assert_eq!(opcua_ticks_from_unix_nanos(nanos), Some(i64::MAX));
        assert_eq!(opcua_ticks_from_unix_nanos(nanos + 100), None);
    }

    #[test]
    fn ticks_that_do_not_fit_in_i64_are_none() {
        let ticks = u128::from(i64::MAX as u64) + 1;
        assert_eq!(opcua_ticks_from_unix_nanos(ticks * 100), None);
    }

    /// `SystemTime` に収まらないナノ秒は `None`。加算で panic しない。
    fn system_time_after_unix_nanos(nanos: u128) -> Option<SystemTime> {
        let secs = u64::try_from(nanos / 1_000_000_000).ok()?;
        let subsec = u32::try_from(nanos % 1_000_000_000).ok()?;
        UNIX_EPOCH.checked_add(Duration::new(secs, subsec))
    }
}
