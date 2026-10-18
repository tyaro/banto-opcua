//! 利用者が入力する NodeId の文字列表現（ADR-0005）。
//!
//! クライアントが読み書きするノードは、名前空間を番号（`ns=`）か URI（`nsu=`）で
//! 指定する。URI の形は接続のたびに NamespaceArray で番号へ解決するので、解析の
//! 結果は [`NodeId`] ではなく、どちらの形も持てる [`NodeAddress`] にする。
//! 文字列表現は OPC 10000-6 の NodeId / ExpandedNodeId に従う。

use std::{fmt, str::FromStr};

use crate::{Identifier, NodeId};

/// 名前空間の指定。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum NamespaceRef {
    /// 名前空間インデックス（`ns=`、省略時は 0）。
    Index(u16),
    /// 名前空間 URI（`nsu=`）。`%3B` と `%25` を復号した後の URI。
    Uri(String),
}

/// 利用者が指定したノード。名前空間を番号か URI で持つ。
///
/// [`FromStr`] で解析し、[`fmt::Display`] で Part 6 の形へ戻す。番号の形は
/// [`NodeId`] と同じく `ns=0;` を省かない。解析は省いた形も受け付ける。
///
/// 表示した文字列を解析すると同じ値に戻る。ただし空の文字列識別子と空の URI は
/// 表示できても解析できない（解析はどちらもエラーにする）。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct NodeAddress {
    /// 名前空間の指定。
    pub namespace: NamespaceRef,
    /// その名前空間の中の識別子。
    pub identifier: Identifier,
}

impl NodeAddress {
    /// 接続先の NamespaceArray で、名前空間を番号に直す。
    ///
    /// 番号の形はそのまま返す（範囲外の番号はサーバーがノード単位の
    /// `BadNodeIdUnknown` にする）。URI が NamespaceArray に無い、または番号が
    /// `u16` に収まらないときは `None`。
    pub fn resolve(&self, namespace_array: &[String]) -> Option<NodeId> {
        let namespace = match &self.namespace {
            NamespaceRef::Index(index) => *index,
            NamespaceRef::Uri(uri) => {
                let position = namespace_array.iter().position(|entry| entry == uri)?;
                u16::try_from(position).ok()?
            }
        };
        Some(NodeId {
            namespace,
            identifier: self.identifier.clone(),
        })
    }
}

impl From<NodeId> for NodeAddress {
    fn from(node_id: NodeId) -> Self {
        Self {
            namespace: NamespaceRef::Index(node_id.namespace),
            identifier: node_id.identifier,
        }
    }
}

/// 解析が当面は対応しない識別子の型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum UnsupportedIdentifierType {
    /// GUID（`g=`）。
    Guid,
    /// ByteString（`b=`）。
    ByteString,
}

/// [`NodeAddress`] の解析の失敗。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum NodeAddressError {
    /// `svr=` の値が 0（このサーバー）でない。別のサーバーは指定できない。
    ServerIndexNotSupported,
    /// `ns=` の値が 0〜65535 の 10 進数でない。
    InvalidNamespaceIndex,
    /// `nsu=` の URI が空。
    InvalidNamespaceUri,
    /// 識別子が無い、または空。
    MissingIdentifier,
    /// `i=` の値が 0〜4294967295 の 10 進数でない。
    InvalidNumericIdentifier,
    /// 識別子の型が `i` / `s` / `g` / `b` のどれでもない。
    UnknownIdentifierType,
    /// 識別子の型が当面は対応しないもの。
    UnsupportedIdentifierType(UnsupportedIdentifierType),
}

impl fmt::Display for NodeAddressError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::ServerIndexNotSupported => "svr= は 0（このサーバー）だけを指定できます",
            Self::InvalidNamespaceIndex => "ns= の値は 0〜65535 の整数で指定してください",
            Self::InvalidNamespaceUri => "nsu= の名前空間 URI が空です",
            Self::MissingIdentifier => "識別子（i= または s=）がありません",
            Self::InvalidNumericIdentifier => "i= の値は 0〜4294967295 の整数で指定してください",
            Self::UnknownIdentifierType => "識別子の型は i= か s= で指定してください",
            Self::UnsupportedIdentifierType(UnsupportedIdentifierType::Guid) => {
                "GUID の識別子（g=）にはまだ対応していません"
            }
            Self::UnsupportedIdentifierType(UnsupportedIdentifierType::ByteString) => {
                "ByteString の識別子（b=）にはまだ対応していません"
            }
        })
    }
}

impl std::error::Error for NodeAddressError {}

impl FromStr for NodeAddress {
    type Err = NodeAddressError;

    /// `[svr=0;][ns=<index>;|nsu=<uri>;](i=<number>|s=<string>)` を解析する。
    ///
    /// 並びは Part 6 の順だけを受け付ける（識別子の後ろに `nsu=` を置く形は
    /// 受け付けない）。`s=` は `;` を含んでも残りすべてを識別子にする。`nsu=` の
    /// URI は Part 6 が符号化する `%3B`（`;`）と `%25`（`%`）だけを、大文字・
    /// 小文字を問わず復号し、ほかの `%` はそのまま残す。`svr=0;` は async-opcua の
    /// ExpandedNodeId の表示が付けるので受け付ける。
    ///
    /// 前後の空白は取り除かない（`s=` の空白は識別子の一部になりうる）。貼り付けた
    /// 文字列の空白は呼び出し側が除く。
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let text = match text.strip_prefix("svr=") {
            Some(rest) => {
                let (server, rest) = rest
                    .split_once(';')
                    .ok_or(NodeAddressError::ServerIndexNotSupported)?;
                if parse_decimal::<u32>(server) != Some(0) {
                    return Err(NodeAddressError::ServerIndexNotSupported);
                }
                rest
            }
            None => text,
        };
        let (namespace, rest) = if let Some(rest) = text.strip_prefix("ns=") {
            let (index, rest) = rest
                .split_once(';')
                .ok_or(NodeAddressError::MissingIdentifier)?;
            let index = parse_decimal(index).ok_or(NodeAddressError::InvalidNamespaceIndex)?;
            (NamespaceRef::Index(index), rest)
        } else if let Some(rest) = text.strip_prefix("nsu=") {
            let (uri, rest) = rest
                .split_once(';')
                .ok_or(NodeAddressError::MissingIdentifier)?;
            if uri.is_empty() {
                return Err(NodeAddressError::InvalidNamespaceUri);
            }
            (NamespaceRef::Uri(unescape_uri(uri)), rest)
        } else {
            (NamespaceRef::Index(0), text)
        };

        if rest.is_empty() {
            return Err(NodeAddressError::MissingIdentifier);
        }
        let (kind, value) = rest
            .split_once('=')
            .ok_or(NodeAddressError::UnknownIdentifierType)?;
        let identifier = match kind {
            "i" => Identifier::Numeric(
                parse_decimal(value).ok_or(NodeAddressError::InvalidNumericIdentifier)?,
            ),
            "s" if value.is_empty() => return Err(NodeAddressError::MissingIdentifier),
            "s" => Identifier::String(value.to_owned()),
            "g" => {
                return Err(NodeAddressError::UnsupportedIdentifierType(
                    UnsupportedIdentifierType::Guid,
                ));
            }
            "b" => {
                return Err(NodeAddressError::UnsupportedIdentifierType(
                    UnsupportedIdentifierType::ByteString,
                ));
            }
            _ => return Err(NodeAddressError::UnknownIdentifierType),
        };
        Ok(Self {
            namespace,
            identifier,
        })
    }
}

impl fmt::Display for NodeAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.namespace {
            NamespaceRef::Index(index) => write!(f, "ns={index};")?,
            NamespaceRef::Uri(uri) => {
                f.write_str("nsu=")?;
                for character in uri.chars() {
                    match character {
                        ';' => f.write_str("%3B")?,
                        '%' => f.write_str("%25")?,
                        _ => write!(f, "{character}")?,
                    }
                }
                f.write_str(";")?;
            }
        }
        match &self.identifier {
            Identifier::Numeric(id) => write!(f, "i={id}"),
            Identifier::String(id) => write!(f, "s={id}"),
        }
    }
}

/// 符号・空白を許さない 10 進数。`u16::from_str` などは先頭の `+` を受け付けるので使わない。
fn parse_decimal<T: FromStr>(text: &str) -> Option<T> {
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

/// `%3B` と `%25` を `;` と `%` に戻す。ほかの `%` は URI 自体の符号化として残す。
fn unescape_uri(text: &str) -> String {
    let mut uri = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(position) = rest.find('%') {
        uri.push_str(&rest[..position]);
        let escape = rest.get(position..position + 3);
        if escape.is_some_and(|escape| escape.eq_ignore_ascii_case("%3B")) {
            uri.push(';');
            rest = &rest[position + 3..];
        } else if escape == Some("%25") {
            uri.push('%');
            rest = &rest[position + 3..];
        } else {
            uri.push('%');
            rest = &rest[position + 1..];
        }
    }
    uri.push_str(rest);
    uri
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Result<NodeAddress, NodeAddressError> {
        text.parse()
    }

    fn index(namespace: u16, identifier: Identifier) -> NodeAddress {
        NodeAddress {
            namespace: NamespaceRef::Index(namespace),
            identifier,
        }
    }

    fn uri(namespace: &str, identifier: Identifier) -> NodeAddress {
        NodeAddress {
            namespace: NamespaceRef::Uri(namespace.to_owned()),
            identifier,
        }
    }

    #[test]
    fn parses_index_forms() {
        assert_eq!(
            parse("ns=2;i=1001"),
            Ok(index(2, Identifier::Numeric(1001)))
        );
        assert_eq!(
            parse("ns=3;s=PLC1.Line1.Speed"),
            Ok(index(3, Identifier::String("PLC1.Line1.Speed".into())))
        );
        assert_eq!(
            parse("ns=65535;i=4294967295"),
            Ok(index(65535, Identifier::Numeric(u32::MAX)))
        );
    }

    #[test]
    fn omitted_namespace_is_zero() {
        assert_eq!(parse("i=2259"), Ok(index(0, Identifier::Numeric(2259))));
        assert_eq!(
            parse("s=Tag"),
            Ok(index(0, Identifier::String("Tag".into())))
        );
    }

    #[test]
    fn string_identifier_takes_the_rest_including_semicolons_and_equals() {
        assert_eq!(
            parse("ns=2;s=a;b=c;ns=4"),
            Ok(index(2, Identifier::String("a;b=c;ns=4".into())))
        );
        assert_eq!(
            parse("s=ns=1;i=2"),
            Ok(index(0, Identifier::String("ns=1;i=2".into())))
        );
    }

    #[test]
    fn parses_uri_forms_and_decodes_semicolon_and_percent_in_either_case() {
        assert_eq!(
            parse("nsu=urn:vendor:a%3Bb;s=Tag"),
            Ok(uri("urn:vendor:a;b", Identifier::String("Tag".into())))
        );
        assert_eq!(
            parse("nsu=urn:vendor:a%3bb%25;i=5"),
            Ok(uri("urn:vendor:a;b%", Identifier::Numeric(5)))
        );
    }

    #[test]
    fn other_percent_sequences_stay_part_of_the_uri() {
        for (text, expected) in [
            ("nsu=urn:my%20ns;i=1", "urn:my%20ns"),
            ("nsu=http://x/%E6%97%A5;i=1", "http://x/%E6%97%A5"),
            ("nsu=a%3;i=1", "a%3"),
            ("nsu=a%;i=1", "a%"),
            ("nsu=a%2525;i=1", "a%25"),
        ] {
            assert_eq!(
                parse(text),
                Ok(uri(expected, Identifier::Numeric(1))),
                "{text}"
            );
        }
    }

    #[test]
    fn accepts_server_index_zero_from_async_opcua_display() {
        assert_eq!(
            parse("svr=0;nsu=urn:a%3bb;i=1"),
            Ok(uri("urn:a;b", Identifier::Numeric(1)))
        );
        assert_eq!(
            parse("svr=0;ns=2;i=1"),
            Ok(index(2, Identifier::Numeric(1)))
        );
        for text in ["svr=1;ns=2;i=1", "svr=;i=1", "svr=x;i=1", "svr=0"] {
            assert_eq!(
                parse(text),
                Err(NodeAddressError::ServerIndexNotSupported),
                "{text}"
            );
        }
    }

    #[test]
    fn display_round_trips_and_keeps_ns0() {
        for text in [
            "ns=0;i=2259",
            "ns=2;s=a;b",
            "nsu=urn:vendor:a%3Bb%25c;s=Tag",
            "nsu=http://opcfoundation.org/UA/;i=2255",
            "nsu=urn:my%2520ns;i=1",
        ] {
            assert_eq!(parse(text).unwrap().to_string(), text);
        }
        assert_eq!(parse("i=2259").unwrap().to_string(), "ns=0;i=2259");
        assert_eq!(parse("nsu=a%3bb;i=1").unwrap().to_string(), "nsu=a%3Bb;i=1");
    }

    #[test]
    fn constructed_values_round_trip_through_display() {
        let awkward = "a;b=c%25%3B;nsu=x;i=1 ";
        for address in [
            index(0, Identifier::String(awkward.into())),
            index(7, Identifier::Numeric(0)),
            uri("urn:a;b%c%3B%25", Identifier::String(awkward.into())),
            uri("http://x/%E6%97%A5 ns", Identifier::Numeric(u32::MAX)),
        ] {
            assert_eq!(
                parse(&address.to_string()),
                Ok(address.clone()),
                "{address}"
            );
        }
    }

    #[test]
    fn empty_string_identifier_and_uri_do_not_round_trip() {
        assert_eq!(
            parse(&index(2, Identifier::String(String::new())).to_string()),
            Err(NodeAddressError::MissingIdentifier)
        );
        assert_eq!(
            parse(&uri("", Identifier::Numeric(1)).to_string()),
            Err(NodeAddressError::InvalidNamespaceUri)
        );
    }

    #[test]
    fn display_matches_node_id_for_index_form() {
        let node_id = NodeId::string(2, "PLC1.Line1.Speed");
        assert_eq!(
            NodeAddress::from(node_id.clone()).to_string(),
            node_id.to_string()
        );
    }

    #[test]
    fn rejects_malformed_namespaces() {
        for text in [
            "ns=;i=1",
            "ns=+1;i=1",
            "ns= 1;i=1",
            "ns=65536;i=1",
            "ns=x;i=1",
        ] {
            assert_eq!(
                parse(text),
                Err(NodeAddressError::InvalidNamespaceIndex),
                "{text}"
            );
        }
        assert_eq!(
            parse("nsu=;i=1"),
            Err(NodeAddressError::InvalidNamespaceUri)
        );
    }

    #[test]
    fn rejects_malformed_identifiers() {
        for text in ["", "ns=2", "ns=2;", "nsu=urn:a", "s="] {
            assert_eq!(
                parse(text),
                Err(NodeAddressError::MissingIdentifier),
                "{text}"
            );
        }
        for text in ["i=", "i=-1", "i=+1", "i=4294967296", "i=1.0", "ns=1;i=0x10"] {
            assert_eq!(
                parse(text),
                Err(NodeAddressError::InvalidNumericIdentifier),
                "{text}"
            );
        }
        for text in ["x=1", "Tag", "ns=1;Tag", "I=1", "S=Tag"] {
            assert_eq!(
                parse(text),
                Err(NodeAddressError::UnknownIdentifierType),
                "{text}"
            );
        }
    }

    #[test]
    fn guid_and_byte_string_are_unsupported_not_malformed() {
        assert_eq!(
            parse("ns=1;g=09087e75-8e5e-499b-954f-f2a9603db28a"),
            Err(NodeAddressError::UnsupportedIdentifierType(
                UnsupportedIdentifierType::Guid
            ))
        );
        assert_eq!(
            parse("b=M/RbKBsRVkePCePcx24oRA=="),
            Err(NodeAddressError::UnsupportedIdentifierType(
                UnsupportedIdentifierType::ByteString
            ))
        );
    }

    #[test]
    fn resolve_maps_uri_to_its_position_in_the_namespace_array() {
        let array = vec![
            "http://opcfoundation.org/UA/".to_owned(),
            "urn:server".to_owned(),
            "urn:vendor:a;b".to_owned(),
        ];
        assert_eq!(
            parse("nsu=urn:vendor:a%3Bb;s=Tag").unwrap().resolve(&array),
            Some(NodeId::string(2, "Tag"))
        );
        assert_eq!(
            parse("nsu=http://opcfoundation.org/UA/;i=2255")
                .unwrap()
                .resolve(&array),
            Some(NodeId::numeric(0, 2255))
        );
        assert_eq!(parse("nsu=urn:missing;i=1").unwrap().resolve(&array), None);
    }

    #[test]
    fn resolve_keeps_index_even_when_out_of_range() {
        assert_eq!(
            parse("ns=9;i=1").unwrap().resolve(&[]),
            Some(NodeId::numeric(9, 1))
        );
    }
}
