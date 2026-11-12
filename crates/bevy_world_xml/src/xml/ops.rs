//! Editing API for [`XmlNode`]/[`XmlDocument`].
//!
//! The bake mutates the owned tree before emitting it: create a node,
//! duplicate a subtree, insert/remove a child and set/get attributes. The
//! model is fully owned (`String`/`Vec`, no roxmltree lifetimes), so editing
//! is direct.

use super::{XmlDocument, XmlNode};

impl XmlNode {
    /// Creates an empty node with the given tag — e.g. the `<Group>` that
    /// replaces a spawner in the bake.
    pub fn new(tag: impl Into<String>) -> Self {
        XmlNode {
            tag: tag.into(),
            attrs: Vec::new(),
            text: String::new(),
            children: Vec::new(),
            origin: None,
        }
    }

    /// Builder: appends an attr and returns the node, for chained literals
    /// (`XmlNode::new("Group").with_attr("name", "trees")`).
    pub fn with_attr(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.attrs.push((name.into(), value.into()));
        self
    }

    /// Sets an attr: replaces the first occurrence's value (keeping its
    /// position, preserving the order of the rest) or appends it.
    pub fn set_attr(&mut self, name: &str, value: impl Into<String>) {
        let value = value.into();
        if let Some(slot) = self.attrs.iter_mut().find(|(k, _)| k == name) {
            slot.1 = value;
        } else {
            self.attrs.push((name.to_string(), value));
        }
    }

    /// Removes the attr; returns `true` when it existed.
    pub fn remove_attr(&mut self, name: &str) -> bool {
        let before = self.attrs.len();
        self.attrs.retain(|(k, _)| k != name);
        self.attrs.len() != before
    }

    /// Appends a child.
    pub fn push_child(&mut self, child: XmlNode) {
        self.children.push(child);
    }

    /// Duplicates the whole subtree (an explicit alias for the derived
    /// `Clone` — the name reads as intent in the bake).
    pub fn duplicate(&self) -> XmlNode {
        self.clone()
    }
}

impl XmlDocument {
    /// New document with the given root tag (the bake uses `"world"`). The
    /// root is emitted as given — the parser lowercases it when reading.
    pub fn new(root_tag: impl Into<String>) -> Self {
        XmlDocument {
            root_tag: root_tag.into(),
            root_attrs: Vec::new(),
            root_text: String::new(),
            children: Vec::new(),
        }
    }

    /// Root attr — same semantics as [`XmlNode::set_attr`].
    pub fn set_root_attr(&mut self, name: &str, value: impl Into<String>) {
        let value = value.into();
        if let Some(slot) = self.root_attrs.iter_mut().find(|(k, _)| k == name) {
            slot.1 = value;
        } else {
            self.root_attrs.push((name.to_string(), value));
        }
    }

    /// Appends a direct child to the root.
    pub fn push_child(&mut self, child: XmlNode) {
        self.children.push(child);
    }
}
