//! OPC UA DateTime と [`std::time::SystemTime`] の変換。
//!
//! OPC UA DateTime は 1601-01-01 UTC からの 100 ナノ秒刻み（符号付き 64 ビット）。
//! `chrono` は引かない（banto ADR-0002 の自前日付変換と同じ理由）。
//!
//! 時刻の方針（2026-10-15 オーナー決定）:
//! Good の SourceTimestamp は採取時刻、Bad / Stale は品質を認識した時刻
//! （または実際に復旧を試みた時刻）。不明なら null。core は渡された時刻を
//! 選択・変換するが、ハブから必要な時刻を受け取る結合は banto-industrial #614
//! の対応後に行う。現在時刻では埋めない。

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::Quality;

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

    /// 品質に応じた SourceTimestamp と、公開時の ServerTimestamp を組み立てる。
    ///
    /// Good は値の採取時刻、Bad / Stale は品質を認識した時刻（または実際に
    /// 復旧を試みた時刻）を選ぶ。選んだ時刻が不明なら、もう一方や `server` へ
    /// フォールバックせず `None` にする。
    /// `quality` は [`crate::map_current_value`] に渡したのと同じハブ品質であり、
    /// 写像後の StatusCode から作り直さない。Good の無効値が Bad に写る場合は
    /// その採取時刻を異常の観測時刻とみなす。採取時刻がなければ `None`。
    /// PLC と Hub ソースの Stale の違いは、呼び出し側が渡す `quality_at` に表れる。
    /// 呼び出し側は品質を認識した時刻を Read のたびに作り直してはならない。
    pub const fn for_quality(
        quality: Quality,
        sampled_at: Option<SystemTime>,
        quality_at: Option<SystemTime>,
        server: SystemTime,
    ) -> Self {
        let source = match quality {
            Quality::Good => sampled_at,
            Quality::Bad | Quality::Stale => quality_at,
        };
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

/// OPC UA DateTime の刻みを `SystemTime` へ変換する。
///
/// 0 以下（null と 1601 年より前）は `None`。`SystemTime` で表せない時刻も
/// `None`（Windows は 30828 年付近まで）。Unix エポックより前の時刻も返す。
pub fn system_time_from_opcua_ticks(ticks: i64) -> Option<SystemTime> {
    if ticks <= 0 {
        return None;
    }
    let since_unix = i128::from(ticks) - i128::from(UNIX_EPOCH_TICKS);
    let magnitude = since_unix.unsigned_abs();
    let duration = Duration::new(
        u64::try_from(magnitude / 10_000_000).ok()?,
        u32::try_from(magnitude % 10_000_000 * 100).ok()?,
    );
    if since_unix >= 0 {
        UNIX_EPOCH.checked_add(duration)
    } else {
        UNIX_EPOCH.checked_sub(duration)
    }
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

    use crate::{DataType, StatusCode, map_current_value};

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
    fn ticks_convert_back_to_system_time() {
        let time = UNIX_EPOCH + Duration::new(1_700_000_000, 123_456_700);
        let ticks = opcua_ticks_from_system_time(time).unwrap();
        assert_eq!(system_time_from_opcua_ticks(ticks), Some(time));
        assert_eq!(
            system_time_from_opcua_ticks(UNIX_EPOCH_TICKS),
            Some(UNIX_EPOCH)
        );
        assert_eq!(
            system_time_from_opcua_ticks(UNIX_EPOCH_TICKS - 10_000_000),
            UNIX_EPOCH.checked_sub(Duration::from_secs(1))
        );
    }

    #[test]
    fn null_and_negative_ticks_have_no_time() {
        assert_eq!(system_time_from_opcua_ticks(0), None);
        assert_eq!(system_time_from_opcua_ticks(-1), None);
    }

    #[test]
    fn missing_source_timestamp_stays_absent() {
        let sample = SampleTimestamps::new(None, UNIX_EPOCH);
        assert_eq!(sample.source_ticks(), None);
        assert_eq!(sample.server_ticks(), Some(UNIX_EPOCH_TICKS));
    }

    #[test]
    fn quality_selects_sample_or_recognition_time_without_fallback() {
        let sampled_at = UNIX_EPOCH + Duration::from_secs(1);
        let quality_at = UNIX_EPOCH + Duration::from_secs(2);
        let server = UNIX_EPOCH + Duration::from_secs(3);
        for (quality, expected) in [
            (Quality::Good, sampled_at),
            (Quality::Bad, quality_at),
            (Quality::Stale, quality_at),
        ] {
            let timestamps =
                SampleTimestamps::for_quality(quality, Some(sampled_at), Some(quality_at), server);
            assert_eq!(timestamps.source, Some(expected), "{quality:?}");
            assert_eq!(timestamps.server, server, "{quality:?}");
        }
        assert_eq!(
            SampleTimestamps::for_quality(Quality::Good, None, Some(quality_at), server)
                .source_ticks(),
            None
        );
        for quality in [Quality::Bad, Quality::Stale] {
            assert_eq!(
                SampleTimestamps::for_quality(quality, Some(sampled_at), None, server)
                    .source_ticks(),
                None,
                "{quality:?}"
            );
        }
    }

    #[test]
    fn changing_server_time_does_not_retimestamp_stale_source() {
        let sampled_at = UNIX_EPOCH + Duration::from_secs(1);
        let quality_at = UNIX_EPOCH + Duration::from_secs(2);
        let first = SampleTimestamps::for_quality(
            Quality::Stale,
            Some(sampled_at),
            Some(quality_at),
            UNIX_EPOCH + Duration::from_secs(3),
        );
        let second = SampleTimestamps::for_quality(
            Quality::Stale,
            Some(sampled_at),
            Some(quality_at),
            UNIX_EPOCH + Duration::from_secs(4),
        );
        assert_eq!(first.source_ticks(), second.source_ticks());
        assert_ne!(first.server_ticks(), second.server_ticks());
    }

    #[test]
    fn invalid_good_value_uses_observation_time_not_unrelated_quality_time() {
        let sampled_at = UNIX_EPOCH + Duration::from_secs(1);
        let quality_at = UNIX_EPOCH + Duration::from_secs(2);
        let server = UNIX_EPOCH + Duration::from_secs(3);
        let mapped = map_current_value(DataType::Double, Some(f64::NAN), Quality::Good);
        assert_eq!(mapped.status, StatusCode::BAD);
        assert_eq!(mapped.value, None);

        let timestamps = SampleTimestamps::for_quality(
            Quality::Good,
            Some(sampled_at),
            Some(quality_at),
            server,
        );
        assert_eq!(timestamps.source, Some(sampled_at));
        assert_ne!(timestamps.source, Some(quality_at));

        let missing = map_current_value(DataType::Double, None, Quality::Good);
        assert_eq!(missing.status, StatusCode::BAD);
        let unknown = SampleTimestamps::for_quality(Quality::Good, None, Some(quality_at), server);
        assert_eq!(unknown.source, None);
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
