//! O manifesto de um plugin: o que ele declara e o que o host pode chamar.
//!
//! O host não conhece regras de jogo. A única coisa que ele valida é a FORMA:
//! ids estáveis, callbacks que existem, snippets de nós que o parser aceita e
//! limites de tamanho.

use mlua::{Function, Table};

/// Versão da API que este host entende.
pub const API_VERSION: u32 = 1;

/// Um painel declarado por um plugin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PanelSpec {
    /// Índice 1-based no array `panels` do manifesto (para buscar `render`).
    pub index: usize,
    /// Id curto, único dentro do plugin.
    pub id: String,
    pub title: String,
}

/// Um comando que entra na paleta Ctrl+K.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandSpec {
    pub id: String,
    pub title: String,
    /// Termos extra para a pesquisa da paleta (PT/EN).
    pub keywords: String,
}

/// Um template para o diálogo Add Node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeTemplateSpec {
    pub tag: String,
    pub label: String,
    pub snippet: String,
    pub group: String,
}

/// O que um plugin declara.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Manifest {
    pub id: String,
    pub title: String,
    pub panels: Vec<PanelSpec>,
    pub commands: Vec<CommandSpec>,
    pub nodes: Vec<NodeTemplateSpec>,
    pub has_start: bool,
    pub has_update: bool,
    pub has_event: bool,
    pub has_command: bool,
    pub has_stop: bool,
    pub has_lint: bool,
}

impl Manifest {
    /// Lê o manifesto de uma tabela Luau. `fallback_id` é o nome do ficheiro.
    pub fn from_lua(table: &Table, fallback_id: &str) -> Result<Self, String> {
        if let Some(api) = table
            .get::<Option<f64>>("api")
            .map_err(|error| format!("api: {error}"))?
            && api != API_VERSION as f64
        {
            return Err(format!(
                "api {api} não suportada (este editor entende a {API_VERSION})"
            ));
        }
        let id = table
            .get::<Option<String>>("id")
            .map_err(|error| format!("id: {error}"))?
            .unwrap_or_else(|| fallback_id.to_owned());
        validate_id(&id, "id do plugin")?;
        let title = table
            .get::<Option<String>>("title")
            .map_err(|error| format!("title: {error}"))?
            .unwrap_or_else(|| id.clone());

        let mut manifest = Self {
            id: id.clone(),
            title,
            has_start: has_function(table, "on_start")?,
            has_update: has_function(table, "on_update")?,
            has_event: has_function(table, "on_event")?,
            has_command: has_function(table, "on_command")?,
            has_stop: has_function(table, "on_stop")?,
            has_lint: has_function(table, "lint")?,
            ..Self::default()
        };

        if let Some(panels) = table
            .get::<Option<Table>>("panels")
            .map_err(|error| format!("panels: {error}"))?
        {
            let len = panels.raw_len();
            for index in 1..=len {
                let panel = panels
                    .raw_get::<Option<Table>>(index)
                    .map_err(|error| format!("panels[{index}]: {error}"))?
                    .ok_or_else(|| format!("panels[{index}] não é uma tabela"))?;
                let panel_id = panel
                    .get::<Option<String>>("id")
                    .map_err(|error| format!("panels[{index}].id: {error}"))?
                    .ok_or_else(|| format!("panels[{index}] sem `id`"))?;
                validate_id(&panel_id, "id do painel")?;
                if manifest.panels.iter().any(|other| other.id == panel_id) {
                    return Err(format!("painel `{panel_id}` duplicado"));
                }
                if panel
                    .get::<Option<Function>>("render")
                    .map_err(|error| format!("panels[{index}].render: {error}"))?
                    .is_none()
                {
                    return Err(format!("painel `{panel_id}` sem `render(ctx)`"));
                }
                let panel_title = panel
                    .get::<Option<String>>("title")
                    .map_err(|error| format!("panels[{index}].title: {error}"))?
                    .unwrap_or_else(|| panel_id.clone());
                manifest.panels.push(PanelSpec {
                    index,
                    id: panel_id,
                    title: panel_title,
                });
            }
        }

        if let Some(commands) = table
            .get::<Option<Table>>("commands")
            .map_err(|error| format!("commands: {error}"))?
        {
            let len = commands.raw_len();
            for index in 1..=len {
                let command = commands
                    .raw_get::<Option<Table>>(index)
                    .map_err(|error| format!("commands[{index}]: {error}"))?
                    .ok_or_else(|| format!("commands[{index}] não é uma tabela"))?;
                let command_id = command
                    .get::<Option<String>>("id")
                    .map_err(|error| format!("commands[{index}].id: {error}"))?
                    .ok_or_else(|| format!("commands[{index}] sem `id`"))?;
                validate_id(&command_id, "id do comando")?;
                if manifest.commands.iter().any(|other| other.id == command_id) {
                    return Err(format!("comando `{command_id}` duplicado"));
                }
                manifest.commands.push(CommandSpec {
                    id: command_id.clone(),
                    title: command
                        .get::<Option<String>>("title")
                        .map_err(|error| format!("commands[{index}].title: {error}"))?
                        .unwrap_or(command_id),
                    keywords: command
                        .get::<Option<String>>("keywords")
                        .map_err(|error| format!("commands[{index}].keywords: {error}"))?
                        .unwrap_or_default(),
                });
            }
        }

        if let Some(nodes) = table
            .get::<Option<Table>>("nodes")
            .map_err(|error| format!("nodes: {error}"))?
        {
            let len = nodes.raw_len();
            for index in 1..=len {
                let node = nodes
                    .raw_get::<Option<Table>>(index)
                    .map_err(|error| format!("nodes[{index}]: {error}"))?
                    .ok_or_else(|| format!("nodes[{index}] não é uma tabela"))?;
                let tag = node
                    .get::<Option<String>>("tag")
                    .map_err(|error| format!("nodes[{index}].tag: {error}"))?
                    .ok_or_else(|| format!("nodes[{index}] sem `tag`"))?;
                let snippet = node
                    .get::<Option<String>>("snippet")
                    .map_err(|error| format!("nodes[{index}].snippet: {error}"))?
                    .ok_or_else(|| format!("nodes[{index}] sem `snippet`"))?;
                if tag.trim().is_empty() {
                    return Err(format!("nodes[{index}] tem `tag` vazia"));
                }
                if viber::xml::parse_fragment(&snippet).is_err() {
                    return Err(format!("nodes[{index}]: snippet de `<{tag}>` não parseia"));
                }
                manifest.nodes.push(NodeTemplateSpec {
                    tag,
                    label: node
                        .get::<Option<String>>("label")
                        .map_err(|error| format!("nodes[{index}].label: {error}"))?
                        .unwrap_or_else(|| "nó".to_owned()),
                    snippet,
                    group: node
                        .get::<Option<String>>("group")
                        .map_err(|error| format!("nodes[{index}].group: {error}"))?
                        .unwrap_or_else(|| "content".to_owned()),
                });
            }
        }

        let has_any = manifest.has_start
            || manifest.has_update
            || manifest.has_event
            || manifest.has_command
            || manifest.has_stop
            || manifest.has_lint
            || !manifest.panels.is_empty()
            || !manifest.nodes.is_empty();
        if !has_any {
            return Err(
                "plugin sem callbacks, painéis, comandos ou templates — nada para fazer".to_owned(),
            );
        }
        Ok(manifest)
    }
}

fn has_function(table: &Table, key: &str) -> Result<bool, String> {
    Ok(table
        .get::<Option<Function>>(key)
        .map_err(|error| format!("{key}: {error}"))?
        .is_some())
}

/// Ids estáveis: letras, dígitos, `_`, `-` e `.`.
pub fn validate_id(id: &str, what: &str) -> Result<(), String> {
    if id.is_empty() {
        return Err(format!("{what} vazio"));
    }
    if !id
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
    {
        return Err(format!(
            "{what} `{id}`: usa letras, dígitos, `_`, `-` ou `.`"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mlua::Lua;

    fn manifest(source: &str) -> Result<Manifest, String> {
        let lua = Lua::new();
        let table: Table = lua.load(source).eval().map_err(|error| error.to_string())?;
        Manifest::from_lua(&table, "fallback")
    }

    #[test]
    fn reads_callbacks_panels_commands_and_nodes() {
        let parsed = manifest(
            r#"
            return {
              id = "quests",
              title = "Missões",
              on_update = function() end,
              on_event = function() end,
              panels = { { id = "main", title = "Q", render = function() return {} end } },
              commands = { { id = "new", title = "Nova" } },
              nodes = { { tag = "ResourceNode", snippet = "<ResourceNode />" } },
            }
            "#,
        )
        .unwrap();
        assert_eq!(parsed.id, "quests");
        assert_eq!(parsed.title, "Missões");
        assert!(parsed.has_update && parsed.has_event);
        assert_eq!(parsed.panels.len(), 1);
        assert_eq!(parsed.panels[0].index, 1);
        assert_eq!(parsed.commands[0].id, "new");
        assert_eq!(parsed.nodes[0].tag, "ResourceNode");
    }

    #[test]
    fn rejects_unknown_api_and_empty_plugins() {
        assert!(manifest("return { api = 2, on_update = function() end }").is_err());
        assert!(manifest("return {}").is_err());
    }

    #[test]
    fn rejects_panel_without_render_and_duplicate_ids() {
        assert!(
            manifest(r#"return { panels = { { id = "x" } } }"#).is_err(),
            "painel sem render"
        );
        assert!(
            manifest(
                r#"return {
                  panels = {
                    { id = "x", render = function() return {} end },
                    { id = "x", render = function() return {} end },
                  }
                }"#
            )
            .is_err(),
            "painel duplicado"
        );
    }

    #[test]
    fn validates_node_snippets() {
        assert!(manifest(r#"return { nodes = { { tag = "X", snippet = "<X" } } }"#).is_err());
    }

    #[test]
    fn invalid_ids_are_rejected() {
        assert!(manifest(r#"return { id = "a b", on_update = function() end }"#).is_err());
    }
}
