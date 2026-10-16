//! ハブの現在値を OPC UA の DataValue へ写す境界。

use opcua::types::{DataValue, DateTime, Variant};

use banto_opcua_core::{DataType, Quality, SampleTimestamps, map_current_value};

/// 動的タグの型、現在値、品質、時刻をまとめた入力。
///
/// `timestamps` は呼び出し側が品質に応じて選んだ時刻を保持する。Read のたびに
/// SourceTimestamp を作り直さない。初回未取得は `value = None` とし、
/// `SampleTimestamps::for_quality` に未知の時刻を `None` で渡す。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TagReading {
    pub data_type: DataType,
    pub value: Option<f64>,
    pub quality: Quality,
    pub timestamps: SampleTimestamps,
}

impl TagReading {
    pub const fn new(
        data_type: DataType,
        value: Option<f64>,
        quality: Quality,
        timestamps: SampleTimestamps,
    ) -> Self {
        Self {
            data_type,
            value,
            quality,
            timestamps,
        }
    }

    pub(crate) fn data_value(self) -> DataValue {
        let mapped = map_current_value(self.data_type, self.value, self.quality);
        DataValue {
            // upstream の Variable::value は Value フィールドを省くと
            // StatusCode も落とす。#17 が解消するまで Null Variant を使う。
            value: Some(match mapped.value {
                Some(banto_opcua_core::MappedValue::Boolean(bit)) => Variant::Boolean(bit),
                Some(banto_opcua_core::MappedValue::Double(number)) => Variant::Double(number),
                None => Variant::Empty,
            }),
            status: Some(mapped.status.code().into()),
            source_timestamp: self.timestamps.source_ticks().and_then(date_time),
            source_picoseconds: None,
            server_timestamp: self.timestamps.server_ticks().and_then(date_time),
            server_picoseconds: None,
        }
    }
}

fn date_time(ticks: i64) -> Option<DateTime> {
    // 刻み 0（1601-01-01）は async-opcua の `DateTime::null()` と同じ「時刻なし」。
    // 値を持つ時刻として渡さず、0 以下は None にする。
    // core の刻みは Unix 時刻以降だけなので、実データの採取時刻はここで落ちない。
    // 上限は core の i64 範囲ではなく OPC UA の最大年（9999）。
    (ticks > 0 && ticks < DateTime::endtimes_ticks()).then(|| DateTime::from(ticks))
}
