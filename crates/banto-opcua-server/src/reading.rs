//! ハブの現在値を OPC UA の DataValue へ写す境界。

use opcua::types::{DataTypeId, DataValue, DateTime, UAString, Variant};

use banto_opcua_core::{DataType, MappedValue, Quality, SampleTimestamps, map_typed_value};

/// 動的タグの型、現在値、品質、時刻をまとめた入力。
///
/// `timestamps` は呼び出し側が品質に応じて選んだ時刻を保持する。Read のたびに
/// SourceTimestamp を作り直さない。初回未取得は `value = None` とし、
/// `SampleTimestamps::for_quality` に未知の時刻を `None` で渡す。
#[derive(Debug, Clone, PartialEq)]
pub struct TagReading {
    pub data_type: DataType,
    /// 型の付いた値。型が `data_type` と違う値や無効な値は、値なし・Bad で公開する
    /// （[`banto_opcua_core::map_typed_value`]）。
    pub value: Option<MappedValue>,
    pub quality: Quality,
    pub timestamps: SampleTimestamps,
}

impl TagReading {
    /// ハブの f64 の値から作る。`data_type` の値へは
    /// [`MappedValue::from_number`] で丸めずに変え、表せない値は値なし・Bad で公開する。
    /// 64 ビットの整数を精度を落とさずに渡すときや String は [`Self::typed`] を使う。
    pub fn new(
        data_type: DataType,
        value: Option<f64>,
        quality: Quality,
        timestamps: SampleTimestamps,
    ) -> Self {
        Self::typed(
            data_type,
            value.and_then(|number| MappedValue::from_number(data_type, number)),
            quality,
            timestamps,
        )
    }

    /// 型の付いた値から作る。
    pub const fn typed(
        data_type: DataType,
        value: Option<MappedValue>,
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
        let mapped = map_typed_value(self.data_type, self.value, self.quality);
        DataValue {
            // upstream の Variable::value は Value フィールドを省くと
            // StatusCode も落とす。#17 が解消するまで Null Variant を使う。
            value: Some(mapped.value.map_or(Variant::Empty, variant)),
            status: Some(mapped.status.code().into()),
            source_timestamp: self.timestamps.source_ticks().and_then(date_time),
            source_picoseconds: None,
            server_timestamp: self.timestamps.server_ticks().and_then(date_time),
            server_picoseconds: None,
        }
    }
}

/// core の値を async-opcua の Variant へ写す。型は 1 対 1 で、変換はしない。
pub(crate) fn variant(value: MappedValue) -> Variant {
    match value {
        MappedValue::Boolean(value) => Variant::Boolean(value),
        MappedValue::Int16(value) => Variant::Int16(value),
        MappedValue::UInt16(value) => Variant::UInt16(value),
        MappedValue::Int32(value) => Variant::Int32(value),
        MappedValue::UInt32(value) => Variant::UInt32(value),
        MappedValue::Int64(value) => Variant::Int64(value),
        MappedValue::UInt64(value) => Variant::UInt64(value),
        MappedValue::Float(value) => Variant::Float(value),
        MappedValue::Double(value) => Variant::Double(value),
        MappedValue::String(value) => Variant::String(UAString::from(value)),
    }
}

/// ノードの DataType 属性に使う標準の DataTypeId。
pub(crate) fn data_type_id(data_type: DataType) -> DataTypeId {
    match data_type {
        DataType::Boolean => DataTypeId::Boolean,
        DataType::Int16 => DataTypeId::Int16,
        DataType::UInt16 => DataTypeId::UInt16,
        DataType::Int32 => DataTypeId::Int32,
        DataType::UInt32 => DataTypeId::UInt32,
        DataType::Int64 => DataTypeId::Int64,
        DataType::UInt64 => DataTypeId::UInt64,
        DataType::Float => DataTypeId::Float,
        DataType::Double => DataTypeId::Double,
        DataType::String => DataTypeId::String,
    }
}

/// ノードの DataType 属性から core の型を引く。公開する型でなければ `None`。
pub(crate) fn data_type_from_id(id: &opcua::types::NodeId) -> Option<DataType> {
    [
        DataType::Boolean,
        DataType::Int16,
        DataType::UInt16,
        DataType::Int32,
        DataType::UInt32,
        DataType::Int64,
        DataType::UInt64,
        DataType::Float,
        DataType::Double,
        DataType::String,
    ]
    .into_iter()
    .find(|data_type| *id == data_type_id(*data_type))
}

/// ノードを作るときの仮の値。Variable はこの値の型から DataType 属性を決める。
pub(crate) fn placeholder(data_type: DataType) -> Variant {
    match data_type {
        DataType::Boolean => Variant::Boolean(false),
        DataType::Int16 => Variant::Int16(0),
        DataType::UInt16 => Variant::UInt16(0),
        DataType::Int32 => Variant::Int32(0),
        DataType::UInt32 => Variant::UInt32(0),
        DataType::Int64 => Variant::Int64(0),
        DataType::UInt64 => Variant::UInt64(0),
        DataType::Float => Variant::Float(0.0),
        DataType::Double => Variant::Double(0.0),
        DataType::String => Variant::String(UAString::from("")),
    }
}

fn date_time(ticks: i64) -> Option<DateTime> {
    // 刻み 0（1601-01-01）は async-opcua の `DateTime::null()` と同じ「時刻なし」。
    // 値を持つ時刻として渡さず、0 以下は None にする。
    // core の刻みは Unix 時刻以降だけなので、実データの採取時刻はここで落ちない。
    // 上限は core の i64 範囲ではなく OPC UA の最大年（9999）。
    (ticks > 0 && ticks < DateTime::endtimes_ticks()).then(|| DateTime::from(ticks))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [DataType; 10] = [
        DataType::Boolean,
        DataType::Int16,
        DataType::UInt16,
        DataType::Int32,
        DataType::UInt32,
        DataType::Int64,
        DataType::UInt64,
        DataType::Float,
        DataType::Double,
        DataType::String,
    ];

    #[test]
    fn placeholder_has_the_node_data_type() {
        for data_type in ALL {
            let id: opcua::types::NodeId = data_type_id(data_type).into();
            assert_eq!(
                placeholder(data_type).data_type().map(|ty| ty.node_id),
                Some(id),
                "{data_type}"
            );
        }
    }

    #[test]
    fn variant_keeps_64_bit_integers() {
        assert_eq!(
            variant(MappedValue::UInt64(u64::MAX)),
            Variant::UInt64(u64::MAX)
        );
        assert_eq!(
            variant(MappedValue::Int64(i64::MIN)),
            Variant::Int64(i64::MIN)
        );
    }
}
