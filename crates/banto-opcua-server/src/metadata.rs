//! タグの属性（ADR-0008）を変数ノードの Description とプロパティに写す。

use opcua::types::{
    ByteString, DataTypeId, EUInformation, ExtensionObject, Identifier, LocalizedText, NodeId,
    Range, UAString, VariableTypeId, Variant,
};

use banto_opcua_core::{
    EngineeringUnits, MetadataShape, TagMetadata, UNECE_UNITS_NAMESPACE_URI, VariableType,
};

/// 変数ノードに付けるプロパティ。BrowseName は名前空間 0 の標準の名前。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Property {
    EngineeringUnits,
    EuRange,
    TrueState,
    FalseState,
}

impl Property {
    pub const ALL: [Self; 4] = [
        Self::EngineeringUnits,
        Self::EuRange,
        Self::TrueState,
        Self::FalseState,
    ];

    pub const fn browse_name(self) -> &'static str {
        match self {
            Self::EngineeringUnits => "EngineeringUnits",
            Self::EuRange => "EURange",
            Self::TrueState => "TrueState",
            Self::FalseState => "FalseState",
        }
    }

    pub fn data_type(self) -> DataTypeId {
        match self {
            Self::EngineeringUnits => DataTypeId::EUInformation,
            Self::EuRange => DataTypeId::Range,
            Self::TrueState | Self::FalseState => DataTypeId::LocalizedText,
        }
    }

    /// その形がこのプロパティを持つか。
    pub fn in_shape(self, shape: MetadataShape) -> bool {
        match self {
            Self::EngineeringUnits => shape.engineering_units,
            Self::EuRange => shape.eu_range,
            Self::TrueState | Self::FalseState => shape.two_state,
        }
    }

    /// 属性から作るプロパティの値。持たないなら `None`。
    pub fn value(self, metadata: &TagMetadata) -> Option<Variant> {
        match self {
            Self::EngineeringUnits => metadata
                .engineering_units
                .as_ref()
                .map(|units| ExtensionObject::from_message(eu_information(units)).into()),
            Self::EuRange => metadata.eu_range.map(|range| {
                ExtensionObject::from_message(Range {
                    low: range.low,
                    high: range.high,
                })
                .into()
            }),
            Self::TrueState => metadata
                .two_state
                .as_ref()
                .map(|labels| LocalizedText::from(labels.true_state.as_str()).into()),
            Self::FalseState => metadata
                .two_state
                .as_ref()
                .map(|labels| LocalizedText::from(labels.false_state.as_str()).into()),
        }
    }
}

/// プロパティの NodeId。文字列のタグ NodeId とも、フォルダーの ByteString
/// （先頭が深さ 1 か 2）とも重ならないよう、先頭 3 の ByteString にする。
pub(crate) fn property_id(
    namespace: u16,
    connection: &str,
    group: &str,
    tag: &str,
    property: Property,
) -> NodeId {
    let mut bytes = vec![3_u8];
    for part in [connection, group, tag, property.browse_name()] {
        let part = part.as_bytes();
        bytes.extend_from_slice(
            &u32::try_from(part.len())
                .expect("名前の長さは u32 以内")
                .to_be_bytes(),
        );
        bytes.extend_from_slice(part);
    }
    NodeId {
        namespace,
        identifier: Identifier::ByteString(ByteString { value: Some(bytes) }),
    }
}

pub(crate) fn description(metadata: &TagMetadata) -> LocalizedText {
    metadata
        .description
        .as_deref()
        .map_or_else(LocalizedText::null, LocalizedText::from)
}

pub(crate) fn variable_type_id(variable_type: VariableType) -> VariableTypeId {
    match variable_type {
        VariableType::BaseDataVariable => VariableTypeId::BaseDataVariableType,
        VariableType::BaseAnalog => VariableTypeId::BaseAnalogType,
        VariableType::AnalogItem => VariableTypeId::AnalogItemType,
        VariableType::TwoStateDiscrete => VariableTypeId::TwoStateDiscreteType,
    }
}

fn eu_information(units: &EngineeringUnits) -> EUInformation {
    let unit_id = units.unit_id().unwrap_or(-1);
    EUInformation {
        namespace_uri: if unit_id == -1 {
            UAString::null()
        } else {
            UAString::from(UNECE_UNITS_NAMESPACE_URI)
        },
        unit_id,
        display_name: LocalizedText::from(units.display_name.as_str()),
        description: units
            .description
            .as_deref()
            .map_or_else(LocalizedText::null, LocalizedText::from),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eu_information_carries_the_unece_namespace_only_with_a_code() {
        let with_code = eu_information(&EngineeringUnits::display("℃").with_unece_code("CEL"));
        assert_eq!(with_code.unit_id, 4_408_652);
        assert_eq!(with_code.namespace_uri.as_ref(), UNECE_UNITS_NAMESPACE_URI);
        let without = eu_information(&EngineeringUnits::display("個"));
        assert_eq!(without.unit_id, -1);
        assert!(without.namespace_uri.is_null());
        assert_eq!(without.display_name.text.as_ref(), "個");
    }

    #[test]
    fn property_ids_do_not_collide_across_name_splits() {
        let a = property_id(2, "A.B", "C", "D", Property::EuRange);
        let b = property_id(2, "A", "B.C", "D", Property::EuRange);
        let c = property_id(2, "A", "B", "C.D", Property::EuRange);
        assert_ne!(a, b);
        assert_ne!(b, c);
        assert_ne!(
            property_id(2, "A", "B", "C", Property::TrueState),
            property_id(2, "A", "B", "C", Property::FalseState)
        );
    }
}
