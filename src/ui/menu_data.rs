//! Engine list sources for the menus.
//!
//! Each function turns engine state into rows of plain strings, which is all a
//! `<UiList>` template consumes. O CONTEÚDO do jogo (missões, mochila,
//! talentos, loja, controlos) é publicado em Lua com `viber.ui.list`; aqui só
//! fica o que a engine sabe sozinha — o build e o relógio do mundo.
//!
//! Rows carry a `status`-style field wherever the stylesheet wants to colour a
//! row differently — `class="row {status}"` in the template is the whole
//! mechanism.

use bevy::prelude::*;

use super::list::{ListRow, UiLists};

/// Refreshes every engine list. Cheap: [`UiLists::set`] only bumps a version
/// when the rows actually differ, and only a bumped version rebuilds elements.
///
/// Gated by `UiModalsOpen`: com o diário fechado não há consumidores, e
/// reconstruir listas por frame era trabalho morto. Corre quando algum modal
/// está aberto ou no frame seguinte a qualquer toggle (`is_changed`), que
/// cobre a abertura por tecla — o handler exclusivo alterna o recurso depois
/// de `Last`, por isso o frame em que o modal fica visível já apanha as
/// listas frescas. Abertura por script (`viber.ui.open`, `UiSet::Script`)
/// mostra dados 1 frame velhos; aceitável — o frame seguinte refresca.
pub fn collect_menu_lists(
    mut lists: ResMut<UiLists>,
    modals: Res<super::modal::UiModalsOpen>,
    data: Res<super::bind::UiData>,
) {
    if !modals.any() && !modals.is_changed() {
        return;
    }
    // Missões, mochila, talentos, loja, controlos e sistema são
    // `viber.ui.list` (o jogo). A engine só publica um fallback do "system"
    // para um mundo mínimo continuar a ter uma tab legível.
    lists.set_engine("system", system_rows(&data));
}

/// The "system" tab: what build this is and what the world is doing.
pub fn system_rows(data: &super::bind::UiData) -> Vec<ListRow> {
    [
        ("Engine", format!("Viber {}", env!("CARGO_PKG_VERSION"))),
        ("Render", "Bevy 0.20 · Rapier 3D".to_string()),
        ("Hora", data.clock.clone()),
    ]
    .into_iter()
    .map(|(key, value)| UiLists::row([("key", key.to_string()), ("value", value)]))
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_system_rows_are_engine_build_and_clock_only() {
        let data = super::super::bind::UiData {
            clock: "07:15".into(),
            ..Default::default()
        };
        let rows = system_rows(&data);
        assert!(rows.iter().any(|r| r["key"] == "Engine"));
        assert!(rows.iter().any(|r| r["value"] == "07:15"));
        // Nível/vida/mundo são do JOGO — não voltam a aparecer hardcoded.
        for game_key in ["Nível", "Vida", "Mundo"] {
            assert!(
                rows.iter().all(|r| r["key"] != game_key),
                "`{game_key}` é conteúdo do jogo"
            );
        }
    }
}
