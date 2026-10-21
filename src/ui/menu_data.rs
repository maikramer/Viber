//! List sources for the menus: quests, bag, skills, shop, controls, system.
//!
//! Each function turns a gameplay resource into rows of plain strings, which is
//! all a `<UiList>` template consumes. Formatting decisions (how a status is
//! spelled, what a price looks like) live here rather than in the XML, so the
//! menu file stays structure and the stylesheet stays presentation.
//!
//! Rows carry a `status`-style field wherever the stylesheet wants to colour a
//! row differently — `class="row {status}"` in the template is the whole
//! mechanism.

use bevy::prelude::*;

use super::list::{ListRow, UiLists};

/// Objective phrased for a player: `"derrotar 5× lobo"`, not `{kind, target}`.
pub fn objective_text(kind: &str, target: &str, count: u32) -> String {
    let what = target_label(target);
    match kind {
        "kill" if count == 1 => format!("derrotar {what}"),
        "kill" => format!("derrotar {count}× {what}"),
        "collect" => format!("reunir {count}× {what}"),
        "visit" if count == 1 => "visitar 1 local".to_string(),
        "visit" => format!("visitar {count} locais"),
        other => format!("{other} {count}× {what}"),
    }
}

/// Alvo de objetivo em palavras de jogador: o id da criatura (nome do
/// script) ou do recurso vira o nome que o mundo lhe dá. Desconhecido =
/// o próprio id sem hífens (nunca desaparece).
pub fn target_label(target: &str) -> String {
    match target {
        "wolf" => "lobo".into(),
        "goblin" => "goblin".into(),
        "bandit" => "bandido".into(),
        "scorpion" => "escorpião".into(),
        "shade" => "sombra".into(),
        "slime" => "slime".into(),
        "bogling" => "bogling".into(),
        "boss" => "o Ogro dos Picos".into(),
        "bog-warden" => "o Guardião do Lodo".into(),
        "sand-worm" => "o Verme das Areias".into(),
        "witch" => "a Bruxa do Bosque".into(),
        "wood" => "madeira".into(),
        "stone" => "pedra".into(),
        "dark-wood" => "madeira escura".into(),
        "bog-moss" => "musgo do pântano".into(),
        other => other.replace(['-', '_'], " "),
    }
}

/// Nome de exposição de um bioma de quest (o `display-name` das
/// `<BiomeRegion>` do simple-rpg); desconhecido = o id sem hífens.
pub fn biome_label(biome: &str) -> String {
    match biome {
        "city" => "Discórdia".into(),
        "dark-forest" => "Floresta Sombria".into(),
        "desert" => "Ermo Rubro".into(),
        "swamp" => "Pântano da Bruma".into(),
        "frozen-peaks" => "Picos Gelados".into(),
        other => other.replace(['-', '_'], " "),
    }
}

/// Item labels; anything unknown falls back to its own id so a new item shows
/// up in the bag instead of vanishing.
pub fn item_label(id: &str) -> String {
    match id {
        "potion" => "Poção".into(),
        "antidote" => "Antídoto".into(),
        "bomb" => "Bomba".into(),
        // As quatro Notas do Coro (história do simple-rpg — STORY.md).
        "nota" => "Nota de Vael".into(),
        "gold" => "Ouro".into(),
        "wood" => "Madeira".into(),
        "stone" => "Pedra".into(),
        other => {
            let mut chars = other.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        }
    }
}

/// The controls reference — the list that used to sit on top of the world.
pub const CONTROLS: [(&str, &str); 17] = [
    ("W A S D", "mover"),
    ("SHIFT", "correr"),
    ("ESPAÇO", "saltar"),
    ("RATO ESQ / J", "atacar"),
    ("C", "esquiva (dash)"),
    ("E", "curar"),
    ("R", "golpe forte"),
    ("L", "guarda / aparar"),
    ("B", "bomba"),
    ("V", "trocar de alvo"),
    ("1 / 2", "poção / antídoto"),
    ("E", "interagir e falar"),
    ("F", "assinar marco da Nota"),
    ("G", "viagem rápida (junto a uma fogueira)"),
    ("Q / ESC", "abrir e fechar este diário"),
    ("1 - 6", "separador do diário (com ele aberto)"),
    ("P", "profiler"),
];

/// Refreshes every menu list. Cheap: [`UiLists::set`] only bumps a version
/// when the rows actually differ, and only a bumped version rebuilds elements.
///
/// Gated by [`UiModalsOpen`]: com o diário fechado não há consumidores, e
/// reconstruir 6 listas por frame era trabalho morto. Corre quando algum
/// modal está aberto ou no frame seguinte a qualquer toggle (`is_changed`),
/// que cobre a abertura por tecla — o handler exclusivo alterna o recurso
/// depois de `Last`, por isso o frame em que o modal fica visível já apanha
/// as listas frescas. Abertura por script (`viber.ui.open`, `UiSet::Script`)
/// mostra dados 1 frame velhos; aceitável — o frame seguinte refresca.
#[allow(clippy::type_complexity)]
pub fn collect_menu_lists(
    mut lists: ResMut<UiLists>,
    modals: Res<super::modal::UiModalsOpen>,
    data: Res<super::bind::UiData>,
) {
    if !modals.any() && !modals.is_changed() {
        return;
    }
    // Missões, mochila, talentos e loja são `viber.ui.list` (o jogo). Aqui
    // ficam só as listas que não dependem de regras: controlos e sistema.
    lists.set_engine("controls", control_rows());
    lists.set_engine("system", system_rows(&data));
}

pub fn control_rows() -> Vec<ListRow> {
    CONTROLS
        .iter()
        .map(|(key, action)| {
            UiLists::row([("key", key.to_string()), ("action", action.to_string())])
        })
        .collect()
}

/// The "system" tab: what build this is and what the world is doing.
pub fn system_rows(data: &super::bind::UiData) -> Vec<ListRow> {
    [
        ("Engine", format!("Viber {}", env!("CARGO_PKG_VERSION"))),
        ("Render", "Bevy 0.19 · Rapier 3D".to_string()),
        ("Mundo", "simple-rpg".to_string()),
        ("Hora", data.clock.clone()),
        ("Nível", data.level.to_string()),
        (
            "Vida",
            format!(
                "{}/{}",
                data.health.max(0.0).round(),
                data.health_max.round()
            ),
        ),
    ]
    .into_iter()
    .map(|(key, value)| UiLists::row([("key", key.to_string()), ("value", value)]))
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_objective_text_reads_like_a_sentence() {
        assert_eq!(objective_text("kill", "wolf", 5), "derrotar 5× lobo");
        assert_eq!(
            objective_text("kill", "bog-warden", 1),
            "derrotar o Guardião do Lodo"
        );
        assert_eq!(objective_text("collect", "wood", 3), "reunir 3× madeira");
        assert_eq!(
            objective_text("visit", "shrine ruins", 2),
            "visitar 2 locais"
        );
        // An objective kind nobody taught us still renders something useful.
        assert_eq!(objective_text("escort", "npc", 1), "escort 1× npc");
        // Unknown targets keep their id, minus the separators.
        assert_eq!(target_label("frost-slime"), "frost slime");
        assert_eq!(biome_label("dark-forest"), "Floresta Sombria");
    }

    #[test]
    fn test_item_label_falls_back_to_a_capitalised_id() {
        assert_eq!(item_label("potion"), "Poção");
        assert_eq!(item_label("mithril"), "Mithril");
        assert_eq!(item_label(""), "");
    }

    #[test]
    fn test_control_rows_cover_the_whole_reference() {
        let rows = control_rows();
        assert_eq!(rows.len(), CONTROLS.len());
        assert!(rows.iter().any(|r| r["action"] == "mover"));
    }
}
