//! Componentes que ligam um documento `.bsn` ao DSL de mundo do Viber.
//!
//! Cada nó XML vira uma entidade Bevy com `WorldNode { tag, attrs, text }`.
//! O loader do jogo reconstrói os `XmlNode` e o `parse_world`/spawner
//! existentes continuam a ser a única implementação da semântica das tags.
//!
//! `Transform`/`Name`/`GltfSource`/luzes nativas são a camada de edição do
//! Jackdaw: o loader traduz essas componentes de volta para atributos XML.

use bevy::prelude::*;

/// Um nó do DSL de mundo, preservado tal como foi autorado.
#[derive(Component, Reflect, Default, Clone, Debug)]
#[reflect(Component, Default, Debug, Clone)]
#[type_path = "bevy_world_xml::bsn"]
pub struct WorldNode {
    /// Tag XML original (`Entity`, `GltfScene`, `Terrain`, `UiRoot`, …).
    pub tag: String,
    /// Atributos crus, na ordem original.
    pub attrs: Vec<WorldProp>,
    /// Texto direto do nó (ex.: conteúdo de `<UiText>`).
    pub text: String,
    /// Ficheiro XML de origem, relativo à raiz do projeto. Vazio num nó novo.
    pub origin_file: String,
    /// Caminho de índices até ao nó dentro do ficheiro de origem.
    pub origin_path: Vec<u32>,
    /// Se o `Transform` da entidade é autoritativo para a pose XML.
    ///
    /// Tags de terreno/UI/cinemática não têm pose de runtime; as restantes
    /// têm e o editor pode movê-las com os gizmos.
    pub pose: bool,
}

/// Um atributo `chave="valor"` do nó.
#[derive(Reflect, Default, Clone, Debug, PartialEq, Eq)]
#[reflect(Default, Debug, Clone)]
#[type_path = "bevy_world_xml::bsn"]
pub struct WorldProp {
    pub key: String,
    pub value: String,
}

impl WorldNode {
    pub fn attr(&self, key: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|prop| prop.key == key)
            .map(|prop| prop.value.as_str())
    }

    pub fn set_attr(&mut self, key: &str, value: impl Into<String>) {
        let value = value.into();
        if let Some(prop) = self.attrs.iter_mut().find(|prop| prop.key == key) {
            prop.value = value;
        } else {
            self.attrs.push(WorldProp {
                key: key.to_string(),
                value,
            });
        }
    }

    pub fn remove_attr(&mut self, key: &str) {
        self.attrs.retain(|prop| prop.key != key);
    }
}

/// Tags cujo `Transform` não deve ser traduzido para atributos de pose.
///
/// São configurações de terreno/UI/cinemática: o runtime lê `at`/`path`/attrs
/// e ignora a pose da entidade.
pub fn tag_uses_transform(tag: &str) -> bool {
    !matches!(
        tag.to_ascii_lowercase().as_str(),
        "terrain"
            | "terrainpad"
            | "lake"
            | "island"
            | "river"
            | "cut"
            | "plateau"
            | "grounddecal"
            | "cliff"
            | "cave"
            | "chamber"
            | "shaft"
            | "arch"
            | "bridge"
            | "rockfeatures"
            | "road"
            | "roadnetwork"
            | "way"
            | "segment"
            | "spawnexclusion"
            | "navmesh"
            | "ambientlight"
            | "biomeregion"
            | "worldborder"
            | "audiomixer"
            | "interiorscene"
            | "cinematic"
            | "shot"
            | "line"
            | "title"
            | "fade"
            | "sound"
            | "uistyle"
            | "uiroot"
            | "uirow"
            | "uicolumn"
            | "uigrid"
            | "uipanel"
            | "uispacer"
            | "uitext"
            | "uiicon"
            | "uibutton"
            | "uibar"
            | "uicooldown"
            | "uicheck"
            | "uislider"
            | "uiinput"
            | "uimodal"
            | "uilist"
            | "uitemplate"
            | "uiempty"
            | "include"
            | "scatter"
            | "prototype"
    )
}
