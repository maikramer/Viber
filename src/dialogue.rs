//! Texto localizado e balão do HUD — SEM intérprete de grafos.
//!
//! Os grafos de diálogo (`dialogues/*.json`) são CONTEÚDO e correm no jogo
//! (Lua, `scripts/townsfolk.lua`): o script lê o JSON com `viber.load_json`,
//! guarda o cursor em `viber.game()` e escreve com `viber.say`. A engine só
//! resolve chaves `@nome` (locale) e desenha o balão.

use std::collections::HashMap;

use bevy::prelude::*;

/// Resolve um texto `@chave` contra a tabela de locale; sem tabela devolve o original.
pub fn localized<'a>(text: &'a str, table: Option<&HashMap<String, String>>) -> String {
    let Some(key) = text.strip_prefix('@') else {
        return text.to_string();
    };
    table
        .and_then(|t| t.get(key).cloned())
        .unwrap_or_else(|| text.to_string())
}

/// Resolve `@chave` linha a linha (balões multi-linha e `viber.say`).
pub fn localized_lines(text: &str, table: Option<&HashMap<String, String>>) -> String {
    text.split('\n')
        .map(|line| localized(line, table))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Balão do HUD (mesma escrita de `viber.say`).
pub fn show_balloon(
    balloons: &mut Query<(&mut Visibility, &mut crate::hud::HudBalloon, &Children)>,
    texts: &mut Query<&mut Text>,
    body: &str,
    secs: f32,
) -> bool {
    let mut found = false;
    for (mut visibility, mut balloon, children) in balloons.iter_mut() {
        found = true;
        balloon.timer = secs;
        *visibility = Visibility::Visible;
        if let Some(child) = children.first()
            && let Ok(mut text) = texts.get_mut(*child)
        {
            text.0 = body.into();
        }
    }
    found
}
