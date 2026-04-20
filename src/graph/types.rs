use std::collections::HashMap;
use std::path::PathBuf;

use rushdown::ast::{Arena, KindData, Meta, NodeRef};
use serde_json::{Map as JsonMap, Value as JsonValue};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct NodeLocation {
    pub(crate) doc_id: usize,
    pub(crate) node: String,
}

impl NodeLocation {
    pub(crate) fn new(doc_id: usize, node_ref: NodeRef) -> Self {
        Self {
            doc_id,
            node: node_ref.to_string(),
        }
    }
}

#[derive(Debug)]
pub(crate) struct IncludeEntry {
    pub(crate) doc_id: usize,
    pub(crate) heading_shift: usize,
    pub(crate) target_path: PathBuf,
}

#[derive(Debug)]
pub(crate) struct FootnoteDefinitionEntry {
    pub(crate) node_ref: NodeRef,
}

#[derive(Debug)]
pub(crate) struct DocumentOccurrence {
    pub(crate) id: usize,
    pub(crate) path: PathBuf,
    pub(crate) source: String,
    pub(crate) arena: Arena,
    pub(crate) doc_ref: NodeRef,
    pub(super) includes: Vec<(NodeRef, Vec<IncludeEntry>)>,
    pub(crate) footnote_definitions: HashMap<String, FootnoteDefinitionEntry>,
}

impl DocumentOccurrence {
    pub(crate) fn document_metadata(&self) -> JsonMap<String, JsonValue> {
        let mut map = JsonMap::new();
        let KindData::Document(doc) = self.arena[self.doc_ref].kind_data() else {
            return map;
        };
        for (key, value) in doc.metadata() {
            map.insert(key.clone(), meta_to_json(value));
        }
        map
    }

    pub(super) fn include_entries(&self, node_ref: NodeRef) -> Option<&[IncludeEntry]> {
        self.includes.iter().find_map(|(include_ref, entries)| {
            (*include_ref == node_ref).then_some(entries.as_slice())
        })
    }

    pub(crate) fn footnote_definition(&self, label: &str) -> Option<&FootnoteDefinitionEntry> {
        self.footnote_definitions.get(label)
    }
}

pub(crate) fn meta_to_json(meta: &Meta) -> JsonValue {
    match meta {
        Meta::Null => JsonValue::Null,
        Meta::Bool(value) => JsonValue::Bool(*value),
        Meta::Int(value) => JsonValue::Number((*value).into()),
        Meta::Float(value) => serde_json::Number::from_f64(*value)
            .map(JsonValue::Number)
            .unwrap_or(JsonValue::Null),
        Meta::String(value) => JsonValue::String(value.clone()),
        Meta::Sequence(values) => JsonValue::Array(values.iter().map(meta_to_json).collect()),
        Meta::Mapping(values) => JsonValue::Object(
            values
                .iter()
                .map(|(key, value)| (key.clone(), meta_to_json(value)))
                .collect(),
        ),
    }
}
