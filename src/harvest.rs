//! DADOS de props destrutíveis/colhíveis — sem regra.
//!
//! A colheita é do JOGO (Lua): alvo, cadência, ferramenta, loot e XP vivem
//! em `scripts/game/harvest.lua` no Vael. A engine só guarda o que o mundo
//! autorou (`destructible=`/`<ResourceNode>`) e o que o spawner precisa para
//! instanciar o prop: golpes, estilo de quebra, presets de partícula e o
//! recurso que ele dá. `viber.destructibles` expõe estes dados ao script.
//!
//! Nenhum sistema daqui corre: não há plugin, não há [J], não há contador de
//! golpes nativo. O script conta os golpes e usa as primitivas genéricas
//! (`viber.burst`, `viber.sound`, `viber.entity_despawn`, `viber.topple`).

use bevy::prelude::*;

use crate::recipes::{BreakStyleSpec, DestructibleSpec};

/// Estilo de quebra (runtime, resolvido do [`DestructibleSpec`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BreakStyle {
    /// Só o burst de partículas (default).
    #[default]
    Burst,
    /// Árvore tomba; toco persiste (o script decide com `viber.topple`).
    Fall,
    /// Rocha estilhaça em pedaços (o script decide com `viber.burst`).
    Shatter,
}

/// Dados autorais de um prop destrutível/colhível.
#[derive(Debug, Clone, Component)]
pub struct Destructible {
    /// Golpes até quebrar.
    pub hits: u32,
    /// Golpes levados até agora. O script decide quando quebra; este campo
    /// existe para o snapshot (`hits_left`), não para uma regra nativa.
    pub hits_taken: u32,
    /// Alcance autorado do golpe (m).
    pub range: f32,
    /// Estilo de quebra.
    pub break_style: BreakStyle,
    /// Preset do burst de break.
    pub preset: String,
    /// Partículas do burst de break.
    pub burst_count: u32,
    /// Preset do burst por golpe.
    pub hit_preset: String,
    /// Partículas do burst por golpe.
    pub hit_burst_count: u32,
    /// Wobble por golpe (o script pode usar `viber.entity_knockback`).
    pub shake_on_hit: bool,
    /// Popup flutuante no break (`None` = sem popup).
    pub popup: Option<String>,
    /// Cor do popup.
    pub popup_color: Color,
    /// `(kind, yield)` do `<ResourceNode>` — o script decide o que fazer.
    pub resource: Option<(String, u32)>,
}

impl Destructible {
    /// Resolve o spec do XML nos valores runtime (defaults da referência).
    pub fn from_spec(spec: &DestructibleSpec) -> Self {
        Self {
            hits: spec.hits.unwrap_or(3).max(1),
            hits_taken: 0,
            range: spec.range.filter(|r| *r > 0.0).unwrap_or(3.5),
            break_style: match spec.break_style {
                BreakStyleSpec::Burst => BreakStyle::Burst,
                BreakStyleSpec::Fall => BreakStyle::Fall,
                BreakStyleSpec::Shatter => BreakStyle::Shatter,
            },
            preset: map_preset(spec.preset.as_deref().unwrap_or("dust")),
            burst_count: spec.burst_count.unwrap_or(60).max(1),
            hit_preset: map_preset(spec.hit_preset.as_deref().unwrap_or("sparks")),
            hit_burst_count: spec.hit_burst_count.unwrap_or(15).max(1),
            shake_on_hit: spec.shake_on_hit,
            popup: spec.popup_text.clone().filter(|t| !t.trim().is_empty()),
            popup_color: spec
                .popup_color
                .map(|[r, g, b]| Color::srgb(r, g, b))
                .unwrap_or(Color::WHITE),
            resource: spec.resource.clone(),
        }
    }
}

/// Mapeia os presets da referência para os presets da biblioteca de
/// partículas local (`particles::preset`); os outros passam intactos.
pub fn map_preset(name: &str) -> String {
    match name {
        "rockshards" => "sparks".to_string(),
        "woodchips" => "leaves".to_string(),
        "dust" => "ground-dust".to_string(),
        other => other.to_string(),
    }
}
