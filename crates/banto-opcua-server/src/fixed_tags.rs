//! Phase 1 の固定タグと、Phase 2 の追加・削除可能なタグ。
//! ハブのタグレジストリとの結合は banto-industrial #201 で行う。

use std::sync::Arc;

use opcua::{
    server::{
        ServerHandle,
        address_space::{ObjectBuilder, Variable},
        diagnostics::NamespaceMetadata,
        node_manager::memory::{SimpleNodeManager, simple_node_manager},
    },
    types::{
        AttributeId, DataValue, DateTime, NodeId, ObjectId, QualifiedName, ReferenceTypeId,
        StatusCode, VariableTypeId, Variant,
    },
};

use crate::Error;

const MANAGER_NAME: &str = "banto-fixed-tags";

/// Phase 1 の固定ノードを扱う node manager を作る。
pub(crate) fn node_manager(
    namespace_uri: String,
) -> impl opcua::server::node_manager::NodeManagerBuilder {
    simple_node_manager(
        NamespaceMetadata {
            namespace_uri,
            ..Default::default()
        },
        MANAGER_NAME,
    )
}

/// 固定タグのノード ID と値更新に必要なハンドル。
pub(crate) struct FixedTags {
    pub namespace_index: u16,
    manager: Arc<SimpleNodeManager>,
}

impl FixedTags {
    /// 起動前に固定階層を登録する。
    pub fn register(handle: &ServerHandle, namespace_uri: &str) -> Result<Self, Error> {
        let namespace_index = handle
            .get_namespace_index(namespace_uri)
            .ok_or_else(|| Error::Build("固定タグの名前空間が登録されていません".to_owned()))?;
        let manager = handle
            .node_managers()
            .get_by_name::<SimpleNodeManager>(MANAGER_NAME)
            .ok_or_else(|| Error::Build("固定タグの node manager がありません".to_owned()))?;
        let banto = NodeId::new(namespace_index, "Banto");
        let plc = NodeId::new(namespace_index, "PLC1");
        let line = NodeId::new(namespace_index, "PLC1.Line1");
        {
            let mut space = manager.address_space().write();
            for (id, name, parent) in [
                (&banto, "Banto", ObjectId::ObjectsFolder.into()),
                (&plc, "PLC1", banto.clone()),
                (&line, "Line1", plc.clone()),
            ] {
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

            for (name, value) in [
                ("Speed", opcua::types::Variant::Double(0.0)),
                ("Running", opcua::types::Variant::Boolean(false)),
            ] {
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
        validate_path(connection, group, tag)?;
        let connection_id = NodeId::new(self.namespace_index, connection);
        let group_id = NodeId::new(self.namespace_index, format!("{connection}.{group}"));
        let tag_id = NodeId::new(self.namespace_index, format!("{connection}.{group}.{tag}"));
        {
            let mut space = self.manager.address_space().write();
            if space.node_exists(&tag_id) {
                return Err(Error::TagUpdate(format!("タグ {tag_id} は登録済みです")));
            }
            for (id, name, parent) in [
                (
                    &connection_id,
                    connection,
                    NodeId::new(self.namespace_index, "Banto"),
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
            if !space.insert::<_, NodeId>(
                Variable::new(
                    &tag_id,
                    QualifiedName::new(self.namespace_index, tag),
                    tag,
                    value.clone(),
                ),
                None,
            ) {
                return Err(Error::TagUpdate(format!("タグ {tag_id} を登録できません")));
            }
            space.insert_reference(&group_id, &tag_id, ReferenceTypeId::HasComponent);
            space.insert_reference(
                &tag_id,
                &VariableTypeId::BaseDataVariableType.into(),
                ReferenceTypeId::HasTypeDefinition,
            );
        }
        self.manager
            .set_value(
                handle.subscriptions(),
                &tag_id,
                None,
                DataValue::new_now(value),
            )
            .map_err(|status| Error::TagUpdate(status.to_string()))
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
        let mut space = self.manager.address_space().write();
        if !space.node_exists(&id) {
            return Err(Error::TagUpdate(format!("タグ {id} が見つかりません")));
        }
        let now = DateTime::now();
        handle.subscriptions().notify_data_change(
            [(
                DataValue {
                    value: Some(Variant::Empty),
                    status: Some(StatusCode::BadNodeIdUnknown),
                    source_timestamp: Some(now),
                    server_timestamp: Some(now),
                    ..Default::default()
                },
                &id,
                AttributeId::Value,
            )]
            .into_iter(),
        );
        space.delete(&id, true);
        Ok(())
    }
}

fn validate_path(connection: &str, group: &str, tag: &str) -> Result<(), Error> {
    if [connection, group, tag]
        .into_iter()
        .any(|part| part.is_empty() || part.contains('.'))
    {
        return Err(Error::TagUpdate(
            "タグの各名前は空にせず、'.' を含めないでください".to_owned(),
        ));
    }
    Ok(())
}
