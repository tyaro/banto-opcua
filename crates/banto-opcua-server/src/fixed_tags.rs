//! Phase 1 の固定タグと、Phase 2 の追加・削除可能なタグ。
//! ハブのタグレジストリとの結合は banto-industrial #201 で行う。
//! タグ削除後も接続・グループのフォルダーは残す。再登録時に同じ階層を使う。

use std::sync::Arc;

use opcua::{
    server::{
        ServerHandle,
        address_space::{AccessLevel, ObjectBuilder, Variable},
    },
    types::{
        AttributeId, ByteString, DataEncoding, DataValue, DateTime, Identifier, NodeId,
        NumericRange, ObjectId, QualifiedName, ReferenceTypeId, StatusCode, TimestampsToReturn,
        VariableTypeId, Variant,
    },
};

use banto_opcua_core::{DataType, TagMetadata};

use opcua::server::address_space::{NodeBase, NodeType};

use crate::{
    Error, TagReading,
    metadata::{self, Property},
    reading,
    tag_manager::{self, TagName, TagNodeManager, WritePath},
};

const MANAGER_NAME: &str = "banto-fixed-tags";
const ROOT_NAME: &str = "Banto";

/// 固定タグと動的タグを置く node manager を作る。`write` が無ければ書き込めない。
pub(crate) fn node_manager(
    namespace_uri: String,
    write: Option<WritePath>,
) -> impl opcua::server::node_manager::NodeManagerBuilder {
    tag_manager::node_manager(namespace_uri, MANAGER_NAME, write)
}

/// 固定タグのノード ID と値更新に必要なハンドル。
pub(crate) struct FixedTags {
    pub namespace_index: u16,
    pub(crate) manager: Arc<TagNodeManager>,
}

struct InitialTagValue<'a> {
    placeholder: Variant,
    data_value: DataValue,
    metadata: &'a TagMetadata,
    data_type: Option<DataType>,
}

impl FixedTags {
    /// 起動前に固定階層を登録する。
    pub fn register(
        handle: &ServerHandle,
        namespace_uri: &str,
        sample_tags: bool,
    ) -> Result<Self, Error> {
        let namespace_index = handle
            .get_namespace_index(namespace_uri)
            .ok_or_else(|| Error::Build("固定タグの名前空間が登録されていません".to_owned()))?;
        let manager = handle
            .node_managers()
            .get_by_name::<TagNodeManager>(MANAGER_NAME)
            .ok_or_else(|| Error::Build("固定タグの node manager がありません".to_owned()))?;
        let banto = NodeId::new(namespace_index, ROOT_NAME);
        {
            let mut space = manager.address_space().write();
            let mut folders = vec![(&banto, ROOT_NAME, ObjectId::ObjectsFolder.into())];
            let plc = NodeId::new(namespace_index, "PLC1");
            let line = NodeId::new(namespace_index, "PLC1.Line1");
            if sample_tags {
                folders.push((&plc, "PLC1", banto.clone()));
                folders.push((&line, "Line1", plc.clone()));
            }
            for (id, name, parent) in folders {
                let inserted =
                    ObjectBuilder::new(id, QualifiedName::new(namespace_index, name), name)
                        .is_folder()
                        .organized_by(parent)
                        .insert(&mut *space);
                if !inserted {
                    return Err(Error::Build(format!(
                        "固定フォルダー {name} を登録できません"
                    )));
                }
            }

            for (name, value) in if sample_tags {
                vec![
                    ("Speed", Variant::Double(0.0)),
                    ("Running", Variant::Boolean(false)),
                ]
            } else {
                Vec::new()
            } {
                let id = NodeId::new(namespace_index, format!("PLC1.Line1.{name}"));
                let inserted = space.insert::<_, NodeId>(
                    Variable::new(&id, QualifiedName::new(namespace_index, name), name, value),
                    None,
                );
                if !inserted {
                    return Err(Error::Build(format!("固定タグ {name} を登録できません")));
                }
                space.insert_reference(&line, &id, ReferenceTypeId::HasComponent);
                space.insert_reference(
                    &id,
                    &VariableTypeId::BaseDataVariableType.into(),
                    ReferenceTypeId::HasTypeDefinition,
                );
            }
        }
        Ok(Self {
            namespace_index,
            manager,
        })
    }

    pub fn update(&self, handle: &ServerHandle, name: &str, value: DataValue) -> Result<(), Error> {
        let id = NodeId::new(self.namespace_index, format!("PLC1.Line1.{name}"));
        self.manager
            .set_value(handle.subscriptions(), &id, None, value)
            .map_err(|status| Error::TagUpdate(status.to_string()))
    }

    /// 外部名の階層に読み取り専用の変数を追加する。既存ノードは上書きしない。
    pub fn add_tag(
        &self,
        handle: &ServerHandle,
        connection: &str,
        group: &str,
        tag: &str,
        value: Variant,
    ) -> Result<(), Error> {
        self.add_tag_with(handle, connection, group, tag, value, || {})
    }

    /// `after_insert` は write ロック中の競合順序を試験するためのフック。
    pub(crate) fn add_tag_with(
        &self,
        handle: &ServerHandle,
        connection: &str,
        group: &str,
        tag: &str,
        value: Variant,
        after_insert: impl FnOnce(),
    ) -> Result<(), Error> {
        self.add_tag_data_value_with(
            handle,
            connection,
            group,
            tag,
            InitialTagValue {
                placeholder: value.clone(),
                data_value: DataValue::new_now(value),
                metadata: &TagMetadata::default(),
                data_type: None,
            },
            after_insert,
        )
    }

    pub fn add_tag_reading(
        &self,
        handle: &ServerHandle,
        connection: &str,
        group: &str,
        tag: &str,
        reading: TagReading,
        metadata: &TagMetadata,
    ) -> Result<(), Error> {
        let data_type = reading.data_type;
        let value = reading.data_value();
        metadata
            .validate(data_type)
            .map_err(|error| Error::TagUpdate(error.to_string()))?;
        let placeholder = reading::placeholder(data_type);
        self.add_tag_data_value_with(
            handle,
            connection,
            group,
            tag,
            InitialTagValue {
                placeholder,
                data_value: value,
                metadata,
                data_type: Some(data_type),
            },
            || {},
        )
    }

    fn add_tag_data_value_with(
        &self,
        handle: &ServerHandle,
        connection: &str,
        group: &str,
        tag: &str,
        initial: InitialTagValue,
        after_insert: impl FnOnce(),
    ) -> Result<(), Error> {
        validate_path(connection, group, tag)?;
        let connection_id = folder_id(self.namespace_index, &[connection]);
        let group_id = folder_id(self.namespace_index, &[connection, group]);
        let tag_id = NodeId::new(self.namespace_index, format!("{connection}.{group}.{tag}"));
        let mut space = self.manager.address_space().write();
        if space.node_exists(&tag_id) {
            return Err(Error::TagUpdate(format!("タグ {tag_id} は登録済みです")));
        }
        for (id, name, parent) in [
            (
                &connection_id,
                connection,
                NodeId::new(self.namespace_index, ROOT_NAME),
            ),
            (&group_id, group, connection_id.clone()),
        ] {
            if !space.node_exists(id)
                && !ObjectBuilder::new(id, QualifiedName::new(self.namespace_index, name), name)
                    .is_folder()
                    .organized_by(parent)
                    .insert(&mut *space)
            {
                return Err(Error::TagUpdate(format!(
                    "フォルダー {id} を登録できません"
                )));
            }
        }
        let mut variable = Variable::new(
            &tag_id,
            QualifiedName::new(self.namespace_index, tag),
            tag,
            initial.placeholder,
        );
        variable.set_data_value(initial.data_value);
        variable.set_description(metadata::description(initial.metadata));
        let variable_type = initial
            .data_type
            .map_or(VariableTypeId::BaseDataVariableType, |data_type| {
                metadata::variable_type_id(initial.metadata.shape(data_type).variable_type)
            });
        let properties: Vec<_> = Property::ALL
            .into_iter()
            .filter_map(|property| Some((property, property.value(initial.metadata)?)))
            .map(|(property, value)| {
                let id =
                    metadata::property_id(self.namespace_index, connection, group, tag, property);
                (id, property, value)
            })
            .collect();
        if properties.iter().any(|(id, _, _)| space.node_exists(id)) {
            return Err(Error::TagUpdate(format!(
                "タグ {tag_id} のプロパティが残っています"
            )));
        }
        if !space.insert::<_, NodeId>(variable, None) {
            return Err(Error::TagUpdate(format!("タグ {tag_id} を登録できません")));
        }
        space.insert_reference(&group_id, &tag_id, ReferenceTypeId::HasComponent);
        space.insert_reference(
            &tag_id,
            &variable_type.into(),
            ReferenceTypeId::HasTypeDefinition,
        );
        let added: Vec<_> = std::iter::once(tag_id.clone())
            .chain(properties.iter().map(|(id, _, _)| id.clone()))
            .collect();
        for (id, property, value) in properties {
            let mut node = Variable::new_data_value(
                &id,
                QualifiedName::new(0, property.browse_name()),
                property.browse_name(),
                property.data_type(),
                None,
                None,
                value,
            );
            // 値は親の意味を決め、書き換わりうる（OPC 10000-3 5.6.2）。
            let level = AccessLevel::CURRENT_READ | AccessLevel::SEMANTIC_CHANGE;
            node.set_access_level(level);
            node.set_user_access_level(level);
            // 直前に存在しないことを確かめたので失敗しない。
            space.insert::<_, NodeId>(node, None);
            space.insert_reference(&tag_id, &id, ReferenceTypeId::HasProperty);
            space.insert_reference(
                &id,
                &VariableTypeId::PropertyType.into(),
                ReferenceTypeId::HasTypeDefinition,
            );
        }
        self.manager.inner().tags().write().insert(
            tag_id.clone(),
            TagName {
                connection: connection.to_owned(),
                group: group.to_owned(),
                tag: tag.to_owned(),
            },
        );
        after_insert();
        // 初期値の保存と通知まで write ロックで排他し、削除・再登録との逆転を防ぐ。
        // 通常の値更新と同じ sampler で監視項目ごとにノードを読み直す。プロパティも
        // 通知し、削除で Bad になった購読を同じ監視項目のまま戻す。
        handle.subscriptions().maybe_notify(
            added.iter().map(|node| (node, AttributeId::Value)),
            |id, attribute, range, encoding| {
                space.find(id)?.as_node().get_attribute(
                    TimestampsToReturn::Both,
                    attribute,
                    range,
                    encoding,
                )
            },
        );
        Ok(())
    }

    pub fn update_tag_reading(
        &self,
        handle: &ServerHandle,
        connection: &str,
        group: &str,
        tag: &str,
        data_type: DataType,
        value: DataValue,
    ) -> Result<(), Error> {
        validate_path(connection, group, tag)?;
        let id = NodeId::new(self.namespace_index, format!("{connection}.{group}.{tag}"));
        let group_id = folder_id(self.namespace_index, &[connection, group]);
        let expected: NodeId = reading::data_type_id(data_type).into();
        let mut space = self.manager.address_space().write();
        if !space.has_reference(&group_id, &id, ReferenceTypeId::HasComponent) {
            return Err(Error::TagUpdate(format!(
                "タグ {id} は指定されたグループに属しません"
            )));
        }
        let Some(NodeType::Variable(variable)) = space.find_mut(&id) else {
            return Err(Error::TagUpdate(format!("タグ {id} が見つかりません")));
        };
        if variable.data_type() != expected {
            return Err(Error::TagUpdate(format!(
                "タグ {id} の DataType が一致しません"
            )));
        }
        variable.set_data_value(value);
        handle.subscriptions().maybe_notify(
            [(&id, AttributeId::Value)].into_iter(),
            |id, attribute, range, encoding| {
                space.find(id)?.as_node().get_attribute(
                    TimestampsToReturn::Both,
                    attribute,
                    range,
                    encoding,
                )
            },
        );
        Ok(())
    }

    /// 登録済みタグの Description とプロパティの値を書き換え、購読者へ通知する。
    /// ノードの形（型定義と持つプロパティ）が変わるなら書き換えずにエラーにする。
    pub fn update_tag_metadata(
        &self,
        handle: &ServerHandle,
        connection: &str,
        group: &str,
        tag: &str,
        metadata: &TagMetadata,
    ) -> Result<(), Error> {
        validate_path(connection, group, tag)?;
        let id = NodeId::new(self.namespace_index, format!("{connection}.{group}.{tag}"));
        let group_id = folder_id(self.namespace_index, &[connection, group]);
        let mut space = self.manager.address_space().write();
        // サンプルタグは外部名を持たないので、属性を付けられない。
        if !space.has_reference(&group_id, &id, ReferenceTypeId::HasComponent)
            || !self.manager.inner().tags().read().contains_key(&id)
        {
            return Err(Error::TagUpdate(format!(
                "タグ {id} は指定されたグループにありません"
            )));
        }
        let Some(NodeType::Variable(variable)) = space.find(&id) else {
            return Err(Error::TagUpdate(format!("タグ {id} が見つかりません")));
        };
        let data_type = reading::data_type_from_id(&variable.data_type()).ok_or_else(|| {
            Error::TagUpdate(format!("タグ {id} の DataType は属性を付けられません"))
        })?;
        metadata
            .validate(data_type)
            .map_err(|error| Error::TagUpdate(error.to_string()))?;
        let shape = metadata.shape(data_type);
        let property_ids = Property::ALL.map(|property| {
            (
                property,
                metadata::property_id(self.namespace_index, connection, group, tag, property),
            )
        });
        let same_shape = space.has_reference(
            &id,
            &metadata::variable_type_id(shape.variable_type).into(),
            ReferenceTypeId::HasTypeDefinition,
        ) && property_ids
            .iter()
            .all(|(property, node)| space.node_exists(node) == property.in_shape(shape));
        if !same_shape {
            return Err(Error::MetadataShapeChanged(id.to_string()));
        }
        if let Some(NodeType::Variable(variable)) = space.find_mut(&id) {
            variable.set_description(metadata::description(metadata));
        }
        let mut changed = vec![(id.clone(), AttributeId::Description)];
        let mut semantics_changed = false;
        for (property, node) in &property_ids {
            let (Some(value), Some(NodeType::Variable(variable))) =
                (property.value(metadata), space.find_mut(node))
            else {
                continue;
            };
            if current_value(variable).value.as_ref() == Some(&value) {
                continue;
            }
            semantics_changed = true;
            variable.set_data_value(DataValue::new_now(value));
            changed.push((node.clone(), AttributeId::Value));
        }
        // 単位・レンジ・文言が変わったら、値の購読者にも SemanticsChanged で知らせる
        // （OPC 10000-8 5.2）。値・品質・時刻はそのまま。次の値の更新でビットは消える。
        if semantics_changed && let Some(NodeType::Variable(variable)) = space.find_mut(&id) {
            let mut value = current_value(variable);
            value.status = Some(
                value
                    .status
                    .unwrap_or(StatusCode::Good)
                    .set_semantics_changed(true),
            );
            variable.set_data_value(value);
            changed.push((id.clone(), AttributeId::Value));
        }
        handle.subscriptions().maybe_notify(
            changed.iter().map(|(node, attribute)| (node, *attribute)),
            |id, attribute, range, encoding| {
                space.find(id)?.as_node().get_attribute(
                    TimestampsToReturn::Both,
                    attribute,
                    range,
                    encoding,
                )
            },
        );
        Ok(())
    }

    /// 削除通知を流してからアドレス空間から外す。MonitoredItem は保持する。
    pub fn remove_tag(
        &self,
        handle: &ServerHandle,
        connection: &str,
        group: &str,
        tag: &str,
    ) -> Result<(), Error> {
        validate_path(connection, group, tag)?;
        let id = NodeId::new(self.namespace_index, format!("{connection}.{group}.{tag}"));
        let group_id = folder_id(self.namespace_index, &[connection, group]);
        let mut space = self.manager.address_space().write();
        if !space.node_exists(&id)
            || !space.has_reference(&group_id, &id, ReferenceTypeId::HasComponent)
        {
            return Err(Error::TagUpdate(format!(
                "タグ {id} は指定されたグループにありません"
            )));
        }
        let now = DateTime::now();
        let properties: Vec<_> = Property::ALL
            .into_iter()
            .map(|property| {
                metadata::property_id(self.namespace_index, connection, group, tag, property)
            })
            .filter(|property| space.node_exists(property))
            .collect();
        let removed = DataValue {
            value: None,
            status: Some(StatusCode::BadNodeIdUnknown),
            source_timestamp: Some(now),
            server_timestamp: Some(now),
            ..Default::default()
        };
        handle.subscriptions().notify_data_change(
            std::iter::once(&id)
                .chain(&properties)
                .map(|node| (removed.clone(), node, AttributeId::Value)),
        );
        for property in &properties {
            space.delete(property, true);
        }
        space.delete(&id, true);
        self.manager.inner().tags().write().remove(&id);
        Ok(())
    }

    /// 動的タグの AccessLevel と UserAccessLevel を切り替える。書き込みハンドラーが
    /// 無いサーバーでは、`writable` でも CurrentRead のままにする。
    pub fn set_tag_writable(
        &self,
        connection: &str,
        group: &str,
        tag: &str,
        writable: bool,
    ) -> Result<(), Error> {
        validate_path(connection, group, tag)?;
        let id = NodeId::new(self.namespace_index, format!("{connection}.{group}.{tag}"));
        let group_id = folder_id(self.namespace_index, &[connection, group]);
        let mut space = self.manager.address_space().write();
        // サンプルタグは外部名を持たないので、書き込み可にできない。
        if !space.has_reference(&group_id, &id, ReferenceTypeId::HasComponent)
            || !self.manager.inner().tags().read().contains_key(&id)
        {
            return Err(Error::TagUpdate(format!(
                "タグ {id} は指定されたグループにありません"
            )));
        }
        let Some(NodeType::Variable(variable)) = space.find_mut(&id) else {
            return Err(Error::TagUpdate(format!("タグ {id} が見つかりません")));
        };
        let level = if writable && self.manager.inner().accepts_writes() {
            AccessLevel::CURRENT_READ | AccessLevel::CURRENT_WRITE
        } else {
            AccessLevel::CURRENT_READ
        };
        variable.set_access_level(level);
        variable.set_user_access_level(level);
        Ok(())
    }
}

fn validate_path(connection: &str, group: &str, tag: &str) -> Result<(), Error> {
    if [connection, group, tag].into_iter().any(str::is_empty) {
        return Err(Error::TagUpdate(
            "タグの各名前は空にしないでください".to_owned(),
        ));
    }
    Ok(())
}

/// ドットを含む名前は従来のフォルダー NodeId と衝突し得る。文字列のタグ
/// NodeId とは異なる opaque ID にして、外部名 `{connection}.{group}.{tag}`
/// の契約を変えずに Browse 階層を保持する。単純名は既存 ID を維持する。
fn folder_id(namespace: u16, parts: &[&str]) -> NodeId {
    if parts.iter().all(|part| !part.contains('.')) && !(parts.len() == 1 && parts[0] == ROOT_NAME)
    {
        return NodeId::new(namespace, parts.join("."));
    }
    let mut bytes = vec![u8::try_from(parts.len()).expect("フォルダーの深さは 1 または 2")];
    for part in parts {
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

/// 変数の保存済みの DataValue（値・StatusCode・時刻）。
fn current_value(variable: &Variable) -> DataValue {
    variable.value(
        TimestampsToReturn::Both,
        &NumericRange::None,
        &DataEncoding::Binary,
        0.0,
    )
}
