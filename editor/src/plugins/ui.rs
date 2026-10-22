//! A árvore de UI declarativa que um plugin devolve de `render(ctx)`.
//!
//! É um modelo fechado e comparável: o host valida a forma (profundidade,
//! número de nós, ids interativos únicos) e o renderer Bevy reconstrói só
//! quando a árvore muda. Nada aqui conhece o jogo.

use std::collections::HashSet;

use mlua::{Table, Value};

const MAX_DEPTH: usize = 32;
const MAX_NODES: usize = 5000;
const MAX_TEXT: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tone {
    #[default]
    Normal,
    Dim,
    Warn,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListItem {
    pub id: String,
    pub label: String,
    pub status: Option<String>,
}

/// Um nó de UI. `PartialEq` é usado para evitar rebuilds desnecessários.
#[derive(Debug, Clone, PartialEq)]
pub enum UiNode {
    Column {
        gap: f32,
        children: Vec<UiNode>,
    },
    Row {
        gap: f32,
        children: Vec<UiNode>,
    },
    Heading(String),
    Label {
        text: String,
        tone: Tone,
    },
    Button {
        id: String,
        text: String,
        primary: bool,
        enabled: bool,
        tip: Option<String>,
    },
    Text {
        id: String,
        label: Option<String>,
        value: String,
        placeholder: String,
    },
    Number {
        id: String,
        label: Option<String>,
        value: f64,
        step: f64,
        min: Option<f64>,
        max: Option<f64>,
    },
    Checkbox {
        id: String,
        label: String,
        value: bool,
    },
    Select {
        id: String,
        label: Option<String>,
        value: String,
        options: Vec<(String, String)>,
    },
    List {
        id: String,
        selected: Option<String>,
        items: Vec<ListItem>,
        height: Option<f32>,
    },
    TextLines {
        id: String,
        label: Option<String>,
        values: Vec<String>,
    },
    Separator,
    Scroll {
        height: Option<f32>,
        children: Vec<UiNode>,
    },
    Spacer(f32),
}

impl UiNode {
    /// A árvore de erro mostrada quando `render(ctx)` falha.
    pub fn error(message: impl Into<String>) -> Self {
        UiNode::Column {
            gap: 4.0,
            children: vec![
                UiNode::Heading("Erro no plugin".to_owned()),
                UiNode::Label {
                    text: message.into(),
                    tone: Tone::Error,
                },
            ],
        }
    }

    /// Lê uma árvore devolvida por um plugin.
    pub fn from_lua(value: Value) -> Result<Self, String> {
        let mut limits = Limits {
            nodes: 0,
            seen_ids: HashSet::new(),
        };
        parse(value, 0, &mut limits)
    }
}

struct Limits {
    nodes: usize,
    seen_ids: HashSet<String>,
}

fn parse(value: Value, depth: usize, limits: &mut Limits) -> Result<UiNode, String> {
    if depth > MAX_DEPTH {
        return Err(format!("ui: profundidade máxima excedida ({MAX_DEPTH})"));
    }
    limits.nodes += 1;
    if limits.nodes > MAX_NODES {
        return Err(format!("ui: demasiados nós (máximo {MAX_NODES})"));
    }
    let Value::Table(table) = value else {
        return Err("ui: cada nó tem de ser uma tabela com `type`".to_owned());
    };
    let kind = string(&table, "type")?.to_ascii_lowercase();
    Ok(match kind.as_str() {
        "column" | "row" => {
            let gap = number(&table, "gap").unwrap_or(4.0).max(0.0) as f32;
            let children = children(&table, depth, limits)?;
            if kind == "column" {
                UiNode::Column { gap, children }
            } else {
                UiNode::Row { gap, children }
            }
        }
        "heading" => UiNode::Heading(text(&table, "text")?),
        "label" => UiNode::Label {
            text: text(&table, "text")?,
            tone: match optional_string(&table, "tone")?.as_deref() {
                Some("dim") => Tone::Dim,
                Some("warn") => Tone::Warn,
                Some("error") => Tone::Error,
                _ => Tone::Normal,
            },
        },
        "button" => UiNode::Button {
            id: interactive_id(&table, limits)?,
            text: text(&table, "text")?,
            primary: boolean(&table, "primary").unwrap_or(false),
            enabled: boolean(&table, "enabled").unwrap_or(true),
            tip: optional_string(&table, "tip")?,
        },
        "text" => UiNode::Text {
            id: interactive_id(&table, limits)?,
            label: optional_string(&table, "label")?,
            value: optional_string(&table, "value")?.unwrap_or_default(),
            placeholder: optional_string(&table, "placeholder")?.unwrap_or_default(),
        },
        "number" => UiNode::Number {
            id: interactive_id(&table, limits)?,
            label: optional_string(&table, "label")?,
            value: number(&table, "value").unwrap_or(0.0),
            step: number(&table, "step").unwrap_or(1.0),
            min: number(&table, "min"),
            max: number(&table, "max"),
        },
        "checkbox" => UiNode::Checkbox {
            id: interactive_id(&table, limits)?,
            label: optional_string(&table, "label")?.unwrap_or_default(),
            value: boolean(&table, "value").unwrap_or(false),
        },
        "select" => UiNode::Select {
            id: interactive_id(&table, limits)?,
            label: optional_string(&table, "label")?,
            value: optional_string(&table, "value")?.unwrap_or_default(),
            options: options(&table)?,
        },
        "list" => UiNode::List {
            id: interactive_id(&table, limits)?,
            selected: optional_string(&table, "selected")?,
            items: list_items(&table)?,
            height: number(&table, "height").map(|value| value as f32),
        },
        "text_lines" | "lines" => UiNode::TextLines {
            id: interactive_id(&table, limits)?,
            label: optional_string(&table, "label")?,
            values: strings(&table, "values")?,
        },
        "separator" => UiNode::Separator,
        "scroll" => UiNode::Scroll {
            height: number(&table, "height").map(|value| value as f32),
            children: children(&table, depth, limits)?,
        },
        "spacer" => UiNode::Spacer(number(&table, "size").unwrap_or(8.0).max(0.0) as f32),
        other => return Err(format!("ui: tipo `{other}` desconhecido")),
    })
}

fn children(table: &Table, depth: usize, limits: &mut Limits) -> Result<Vec<UiNode>, String> {
    let Some(children) = table
        .get::<Option<Table>>("children")
        .map_err(|error| format!("ui.children: {error}"))?
    else {
        return Ok(Vec::new());
    };
    let len = children.raw_len();
    let mut out = Vec::with_capacity(len);
    for index in 1..=len {
        let child = children
            .raw_get::<Option<Value>>(index)
            .map_err(|error| format!("ui.children[{index}]: {error}"))?
            .ok_or_else(|| format!("ui.children[{index}] a nil"))?;
        out.push(parse(child, depth + 1, limits)?);
    }
    Ok(out)
}

fn interactive_id(table: &Table, limits: &mut Limits) -> Result<String, String> {
    let id = string(table, "id")?;
    if id.len() > 128 {
        return Err("ui: `id` demasiado longo".to_owned());
    }
    if !limits.seen_ids.insert(id.clone()) {
        return Err(format!("ui: `id` interativo duplicado: {id}"));
    }
    Ok(id)
}

fn options(table: &Table) -> Result<Vec<(String, String)>, String> {
    let Some(options) = table
        .get::<Option<Table>>("options")
        .map_err(|error| format!("ui.options: {error}"))?
    else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for index in 1..=options.raw_len() {
        let value = options
            .raw_get::<Option<Value>>(index)
            .map_err(|error| format!("ui.options[{index}]: {error}"))?
            .ok_or_else(|| format!("ui.options[{index}] a nil"))?;
        match value {
            Value::String(text) => {
                let text = text.to_str().map_err(|_| "ui: opção inválida")?.to_owned();
                out.push((text.clone(), text));
            }
            Value::Table(entry) => {
                let value = string(&entry, "value")?;
                let label = optional_string(&entry, "label")?.unwrap_or_else(|| value.clone());
                out.push((value, label));
            }
            _ => return Err(format!("ui.options[{index}]: esperava string ou tabela")),
        }
    }
    Ok(out)
}

fn list_items(table: &Table) -> Result<Vec<ListItem>, String> {
    let Some(items) = table
        .get::<Option<Table>>("items")
        .map_err(|error| format!("ui.items: {error}"))?
    else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for index in 1..=items.raw_len() {
        let value = items
            .raw_get::<Option<Value>>(index)
            .map_err(|error| format!("ui.items[{index}]: {error}"))?
            .ok_or_else(|| format!("ui.items[{index}] a nil"))?;
        let item = match value {
            Value::String(text) => {
                let text = text.to_str().map_err(|_| "ui: item inválido")?.to_owned();
                ListItem {
                    id: text.clone(),
                    label: text,
                    status: None,
                }
            }
            Value::Table(entry) => ListItem {
                id: string(&entry, "id")?,
                label: optional_string(&entry, "label")?
                    .unwrap_or_else(|| string(&entry, "id").unwrap_or_default()),
                status: optional_string(&entry, "status")?,
            },
            _ => return Err(format!("ui.items[{index}]: esperava string ou tabela")),
        };
        out.push(item);
    }
    Ok(out)
}

fn strings(table: &Table, key: &str) -> Result<Vec<String>, String> {
    let Some(values) = table
        .get::<Option<Table>>(key)
        .map_err(|error| format!("ui.{key}: {error}"))?
    else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for index in 1..=values.raw_len() {
        let value = values
            .raw_get::<Option<String>>(index)
            .map_err(|error| format!("ui.{key}[{index}]: {error}"))?
            .unwrap_or_default();
        if value.len() > MAX_TEXT {
            return Err(format!("ui.{key}[{index}]: texto demasiado longo"));
        }
        out.push(value);
    }
    Ok(out)
}

fn string(table: &Table, key: &str) -> Result<String, String> {
    optional_string(table, key)?.ok_or_else(|| format!("ui: falta `{key}`"))
}

fn optional_string(table: &Table, key: &str) -> Result<Option<String>, String> {
    let value = table
        .get::<Option<Value>>(key)
        .map_err(|error| format!("ui.{key}: {error}"))?;
    let Some(value) = value else {
        return Ok(None);
    };
    match value {
        Value::String(text) => {
            let text = text
                .to_str()
                .map_err(|_| format!("ui.{key}: bytes inválidos"))?
                .to_owned();
            if text.len() > MAX_TEXT {
                return Err(format!("ui.{key}: texto demasiado longo"));
            }
            Ok(Some(text))
        }
        Value::Integer(number) => Ok(Some(number.to_string())),
        Value::Number(number) => Ok(Some(number.to_string())),
        _ => Err(format!("ui.{key}: esperava string")),
    }
}

fn text(table: &Table, key: &str) -> Result<String, String> {
    optional_string(table, key)?.ok_or_else(|| format!("ui: falta `{key}`"))
}

fn number(table: &Table, key: &str) -> Option<f64> {
    table.get::<Option<f64>>(key).ok().flatten()
}

fn boolean(table: &Table, key: &str) -> Option<bool> {
    table.get::<Option<bool>>(key).ok().flatten()
}

#[cfg(test)]
mod tests {
    use super::*;
    use mlua::Lua;

    fn tree(source: &str) -> Result<UiNode, String> {
        let lua = Lua::new();
        let value: Value = lua.load(source).eval().map_err(|error| error.to_string())?;
        UiNode::from_lua(value)
    }

    #[test]
    fn parses_the_widget_set() {
        let node = tree(
            r#"
            return {
              type = "column", gap = 6, children = {
                { type = "heading", text = "Missões" },
                { type = "row", children = {
                  { type = "button", id = "save", text = "Gravar", primary = true },
                  { type = "text", id = "title", label = "Título", value = "Q" },
                } },
                { type = "select", id = "kind", value = "kill",
                  options = { { value = "kill", label = "Matar" }, "visit" } },
                { type = "list", id = "quests", selected = "q1",
                  items = { { id = "q1", label = "Lobos", status = "active" }, "q2" } },
                { type = "text_lines", id = "lines", values = { "a", "b" } },
                { type = "separator" },
              }
            }
            "#,
        )
        .unwrap();
        let UiNode::Column { children, .. } = node else {
            panic!("esperava coluna");
        };
        assert_eq!(children.len(), 6);
    }

    #[test]
    fn rejects_unknown_types_and_duplicate_ids() {
        assert!(tree(r#"return { type = "grid" }"#).is_err());
        assert!(
            tree(
                r#"return { type = "column", children = {
                  { type = "button", id = "x", text = "A" },
                  { type = "button", id = "x", text = "B" },
                } }"#
            )
            .is_err()
        );
    }

    #[test]
    fn rejects_deep_trees() {
        let mut source = String::from("return ");
        for _ in 0..40 {
            source.push_str("{ type = 'column', children = { ");
        }
        source.push_str("{ type = 'label', text = 'x' }");
        for _ in 0..40 {
            source.push_str(" } }");
        }
        assert!(tree(&source).is_err());
    }
}
