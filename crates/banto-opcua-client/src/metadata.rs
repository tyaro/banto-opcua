//! ブラウズしたノードの説明・単位・レンジ・二値の文言（ADR-0008）を、core の
//! [`TagMetadata`] へ写す。async-opcua の型はこのクレートの外へ出さない。

use banto_opcua_core::{
    EngineeringUnits, EuRange, StatusCode, TagMetadata, TwoStateLabels, UNECE_UNITS_NAMESPACE_URI,
};
use opcua::types::{DataValue, EUInformation, LocalizedText, Range, Variant};

/// 1 ノードぶんの属性の読み取り結果。
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct NodeMetadata {
    /// ノードの Description 属性を読んだ StatusCode。Bad ならノードが無いなどで、
    /// `metadata` は空。名前空間 URI を解決できなかったノードは、送らずに `BadNodeIdUnknown`。
    pub status: StatusCode,
    /// 読めた属性。無いプロパティ、読めなかったプロパティ、型の違う値は `None`。
    pub metadata: TagMetadata,
}

impl NodeMetadata {
    pub(crate) fn without_metadata(status: StatusCode) -> Self {
        Self {
            status,
            metadata: TagMetadata::new(),
        }
    }
}

/// 変数ノードのプロパティのうち、読むもの。BrowseName は名前空間 0。
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
}

/// Description 属性の値。空や null の LocalizedText は `None`。
pub(crate) fn description(value: &DataValue) -> Option<String> {
    localized_text(value)
}

/// プロパティの値を属性へ足す。型が違う値や Bad の値は足さない。
/// TrueState と FalseState は両方そろったときだけ足す。
pub(crate) fn apply(
    metadata: &mut TagMetadata,
    values: impl IntoIterator<Item = (Property, DataValue)>,
) {
    let (mut true_state, mut false_state) = (None, None);
    for (property, value) in values {
        if value.status.is_some_and(|status| !status.is_good()) {
            continue;
        }
        match property {
            Property::EngineeringUnits => {
                metadata.engineering_units = extension::<EUInformation>(&value).map(units);
            }
            Property::EuRange => {
                metadata.eu_range =
                    extension::<Range>(&value).map(|range| EuRange::new(range.low, range.high));
            }
            Property::TrueState => true_state = localized_text(&value),
            Property::FalseState => false_state = localized_text(&value),
        }
    }
    if let (Some(true_state), Some(false_state)) = (true_state, false_state) {
        metadata.two_state = Some(TwoStateLabels::new(true_state, false_state));
    }
}

fn extension<T: Clone + Send + Sync + 'static>(value: &DataValue) -> Option<T> {
    match &value.value {
        Some(Variant::ExtensionObject(object)) => object.inner_as::<T>().cloned(),
        _ => None,
    }
}

fn localized_text(value: &DataValue) -> Option<String> {
    match &value.value {
        Some(Variant::LocalizedText(text)) => text_of(text),
        _ => None,
    }
}

fn text_of(text: &LocalizedText) -> Option<String> {
    text.text
        .value()
        .as_deref()
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

/// EUInformation を単位へ写す。UNECE の名前空間で、UnitId が 1〜3 文字の英大文字・
/// 数字のコードに戻るときだけ `unece_code` を持たせる。
fn units(information: EUInformation) -> EngineeringUnits {
    let mut units =
        EngineeringUnits::display(text_of(&information.display_name).unwrap_or_default());
    if let Some(description) = text_of(&information.description) {
        units = units.with_description(description);
    }
    if information.namespace_uri.as_ref() == UNECE_UNITS_NAMESPACE_URI
        && let Some(code) = unece_code(information.unit_id)
    {
        units = units.with_unece_code(code);
    }
    units
}

fn unece_code(unit_id: i32) -> Option<String> {
    let code: String = unit_id
        .to_be_bytes()
        .into_iter()
        .skip_while(|byte| *byte == 0)
        .map(char::from)
        .collect();
    let candidate = EngineeringUnits::display("").with_unece_code(code.clone());
    (candidate.unit_id() == Some(unit_id)).then_some(code)
}

#[cfg(test)]
mod tests {
    use opcua::types::{ExtensionObject, StatusCode as UaStatusCode, UAString};

    use super::*;

    fn value(variant: impl Into<Variant>) -> DataValue {
        DataValue::value_only(variant)
    }

    #[test]
    fn unece_units_keep_the_code_and_other_units_only_the_text() {
        let mut metadata = TagMetadata::new();
        apply(
            &mut metadata,
            [(
                Property::EngineeringUnits,
                value(ExtensionObject::from_message(EUInformation {
                    namespace_uri: UNECE_UNITS_NAMESPACE_URI.into(),
                    unit_id: 4_408_652,
                    display_name: "℃".into(),
                    description: "degree Celsius".into(),
                })),
            )],
        );
        assert_eq!(
            metadata.engineering_units,
            Some(
                EngineeringUnits::display("℃")
                    .with_description("degree Celsius")
                    .with_unece_code("CEL")
            )
        );

        apply(
            &mut metadata,
            [(
                Property::EngineeringUnits,
                value(ExtensionObject::from_message(EUInformation {
                    namespace_uri: UAString::from("urn:other"),
                    unit_id: 4_408_652,
                    display_name: "℃".into(),
                    description: LocalizedText::null(),
                })),
            )],
        );
        assert_eq!(
            metadata.engineering_units,
            Some(EngineeringUnits::display("℃")),
            "UNECE 以外の名前空間の UnitId はコードにしない"
        );
    }

    #[test]
    fn two_state_needs_both_labels_and_bad_or_wrong_values_are_skipped() {
        let mut metadata = TagMetadata::new();
        apply(
            &mut metadata,
            [(Property::TrueState, value(LocalizedText::from("運転")))],
        );
        assert_eq!(metadata.two_state, None);
        apply(
            &mut metadata,
            [
                (Property::TrueState, value(LocalizedText::from("運転"))),
                (Property::FalseState, value(LocalizedText::from("停止"))),
                (Property::EuRange, value(1.0)),
                (
                    Property::EngineeringUnits,
                    DataValue {
                        status: Some(UaStatusCode::BadNotReadable),
                        ..Default::default()
                    },
                ),
            ],
        );
        assert_eq!(
            metadata.two_state,
            Some(TwoStateLabels::new("運転", "停止"))
        );
        assert_eq!(metadata.eu_range, None, "Range でない値は足さない");
        assert_eq!(metadata.engineering_units, None);
    }

    #[test]
    fn unece_code_round_trips_only_valid_codes() {
        assert_eq!(unece_code(4_408_652).as_deref(), Some("CEL"));
        assert_eq!(unece_code(-1), None);
        assert_eq!(unece_code(0x61_62_63), None, "小文字はコードにしない");
    }
}
