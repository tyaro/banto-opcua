//! async-opcua の型と core の語彙の写像。async-opcua の型はこのクレートの外へ出さない。

use std::time::SystemTime;

use banto_opcua_core::{Identifier, NodeId, ScalarValue, StatusCode, system_time_from_opcua_ticks};
use opcua::types::{DataValue, DateTime, NodeId as UaNodeId, StatusCode as UaStatusCode, Variant};

/// 1 ノードぶんの Read の結果。
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct NodeReading {
    /// 値。値が無い、または扱えない値なら `None`。
    pub value: Option<ScalarValue>,
    /// サーバーが返した StatusCode。
    ///
    /// 扱えない値（配列や対応しない型）は `BadNotSupported` に置き換える。
    /// 名前空間 URI を解決できなかったノードは、送らずに `BadNodeIdUnknown`。
    pub status: StatusCode,
    /// サーバーが付けた SourceTimestamp。付いていなければ `None`（現在時刻で補わない）。
    pub source_timestamp: Option<SystemTime>,
}

impl NodeReading {
    /// 結果を組み立てる。呼び出し側の試験で、部品を通さずに結果を作るときに使う。
    pub const fn new(
        value: Option<ScalarValue>,
        status: StatusCode,
        source_timestamp: Option<SystemTime>,
    ) -> Self {
        Self {
            value,
            status,
            source_timestamp,
        }
    }

    /// 送らずに結果を決めたノード。
    pub(crate) const fn without_value(status: StatusCode) -> Self {
        Self {
            value: None,
            status,
            source_timestamp: None,
        }
    }

    pub(crate) fn from_data_value(data_value: DataValue) -> Self {
        let status = data_value.status.map_or(StatusCode::GOOD, core_status_code);
        let source_timestamp = data_value.source_timestamp.as_ref().and_then(system_time);
        match data_value
            .value
            .map(scalar_from_variant)
            .unwrap_or(Ok(None))
        {
            Ok(value) => Self {
                value,
                status,
                source_timestamp,
            },
            Err(Unsupported) => Self {
                value: None,
                status: if status.is_bad() {
                    status
                } else {
                    StatusCode::BAD_NOT_SUPPORTED
                },
                source_timestamp,
            },
        }
    }
}

/// [`ScalarValue`] で表せない値。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Unsupported;

/// `Variant` を core の値へ写す。`Empty` と null の String は値なし。
pub(crate) fn scalar_from_variant(variant: Variant) -> Result<Option<ScalarValue>, Unsupported> {
    Ok(Some(match variant {
        Variant::Empty => return Ok(None),
        Variant::Boolean(value) => ScalarValue::Boolean(value),
        Variant::SByte(value) => ScalarValue::SByte(value),
        Variant::Byte(value) => ScalarValue::Byte(value),
        Variant::Int16(value) => ScalarValue::Int16(value),
        Variant::UInt16(value) => ScalarValue::UInt16(value),
        Variant::Int32(value) => ScalarValue::Int32(value),
        Variant::UInt32(value) => ScalarValue::UInt32(value),
        Variant::Int64(value) => ScalarValue::Int64(value),
        Variant::UInt64(value) => ScalarValue::UInt64(value),
        Variant::Float(value) => ScalarValue::Float(value),
        Variant::Double(value) => ScalarValue::Double(value),
        Variant::String(value) => match value.value() {
            Some(text) => ScalarValue::String(text.clone()),
            None => return Ok(None),
        },
        _ => return Err(Unsupported),
    }))
}

pub(crate) fn ua_node_id(node_id: &NodeId) -> UaNodeId {
    match &node_id.identifier {
        Identifier::Numeric(id) => UaNodeId::new(node_id.namespace, *id),
        Identifier::String(id) => UaNodeId::new(node_id.namespace, id.clone()),
    }
}

pub(crate) fn core_status_code(status: UaStatusCode) -> StatusCode {
    StatusCode::from_code(status.bits())
}

fn system_time(time: &DateTime) -> Option<SystemTime> {
    system_time_from_opcua_ticks(time.ticks())
}

#[cfg(test)]
mod tests {
    use opcua::types::{UAString, Variant};

    use super::*;

    #[test]
    fn scalar_variants_map_to_the_same_built_in_type() {
        let cases = [
            (Variant::Boolean(true), ScalarValue::Boolean(true)),
            (Variant::SByte(-1), ScalarValue::SByte(-1)),
            (Variant::Byte(1), ScalarValue::Byte(1)),
            (Variant::Int16(-2), ScalarValue::Int16(-2)),
            (Variant::UInt16(2), ScalarValue::UInt16(2)),
            (Variant::Int32(-3), ScalarValue::Int32(-3)),
            (Variant::UInt32(3), ScalarValue::UInt32(3)),
            (Variant::Int64(-4), ScalarValue::Int64(-4)),
            (Variant::UInt64(4), ScalarValue::UInt64(4)),
            (Variant::Float(1.5), ScalarValue::Float(1.5)),
            (Variant::Double(f64::NAN), ScalarValue::Double(f64::NAN)),
            (
                Variant::String(UAString::from("a")),
                ScalarValue::String("a".into()),
            ),
        ];
        for (variant, expected) in cases {
            let actual = scalar_from_variant(variant).unwrap().unwrap();
            match (&actual, &expected) {
                (ScalarValue::Double(a), ScalarValue::Double(b)) if b.is_nan() => {
                    assert!(a.is_nan());
                }
                _ => assert_eq!(actual, expected),
            }
        }
    }

    #[test]
    fn empty_and_null_string_have_no_value() {
        assert_eq!(scalar_from_variant(Variant::Empty), Ok(None));
        assert_eq!(
            scalar_from_variant(Variant::String(UAString::null())),
            Ok(None)
        );
    }

    #[test]
    fn arrays_and_other_types_are_unsupported() {
        assert_eq!(
            scalar_from_variant(Variant::from(vec![1i32, 2])),
            Err(Unsupported)
        );
        assert_eq!(
            scalar_from_variant(Variant::from(DateTime::now())),
            Err(Unsupported)
        );
    }

    #[test]
    fn unsupported_value_becomes_bad_not_supported_unless_already_bad() {
        let reading = NodeReading::from_data_value(DataValue::new_now(vec![1i32, 2]));
        assert_eq!(reading.value, None);
        assert_eq!(reading.status, StatusCode::BAD_NOT_SUPPORTED);

        let mut data_value = DataValue::new_now(vec![1i32, 2]);
        data_value.status = Some(UaStatusCode::BadOutOfService);
        assert_eq!(
            NodeReading::from_data_value(data_value).status.code(),
            UaStatusCode::BadOutOfService.bits()
        );
    }

    #[test]
    fn missing_status_is_good_and_missing_timestamp_stays_absent() {
        let data_value = DataValue {
            value: Some(Variant::Double(1.0)),
            ..Default::default()
        };
        let reading = NodeReading::from_data_value(data_value);
        assert_eq!(reading.value, Some(ScalarValue::Double(1.0)));
        assert_eq!(reading.status, StatusCode::GOOD);
        assert_eq!(reading.source_timestamp, None);
    }

    #[test]
    fn source_timestamp_is_converted() {
        let now = DateTime::now();
        let data_value = DataValue {
            value: Some(Variant::Boolean(true)),
            source_timestamp: Some(now),
            ..Default::default()
        };
        let reading = NodeReading::from_data_value(data_value);
        assert_eq!(
            reading
                .source_timestamp
                .and_then(banto_opcua_core::opcua_ticks_from_system_time),
            Some(now.ticks())
        );
    }
}
