//! Posse de SISTEMAS nativos por scripts (`viber.own_system`).
//!
//! Mesmo padrão do `UiActionOwners`: um script reclama um sistema e o handler
//! nativo correspondente passa a saltar — a lógica vive no Lua. SEM posse o
//! comportamento nativo é o de sempre (compatível: nada muda até um script
//! pedir).
//!
//! Só [`SYSTEM_LOCOMOTION`] tem consumidor na engine (`player_movement`): os
//! restantes nomes são LEGADO de quando o corpo do herói era implementado em
//! Rust (diálogo, habilidades, bomba, guarda, hotbar, colheita). A lógica
//! desses sistemas vive hoje nos scripts do jogo, mas os nomes continuam
//! ACEITES para os mundos existentes não falharem no load — reclamá-los já
//! não muda nada. A validação de `viber.own_system`/`release_system` usa
//! [`KNOWN_SYSTEMS`], por isso a lista não pode encolher sem migrar os mundos.

use std::collections::HashSet;

use bevy::prelude::*;

/// Movimento a pé do herói (`player_movement`). Um script que reclama escreve
/// a pose com `viber.set_pose` (kart, veículo, cutscene).
pub const SYSTEM_LOCOMOTION: &str = "locomotion";

/// Nomes aceites por `viber.own_system` / `viber.release_system`.
///
/// Os nomes fora de [`SYSTEM_LOCOMOTION`] são aceites por compatibilidade e
/// já não têm handler nativo para calar (ver a doc do módulo).
pub const KNOWN_SYSTEMS: &[&str] = &[
    "dialogue",
    "abilities",
    "bomb",
    "guard",
    "hotbar",
    "harvest",
    SYSTEM_LOCOMOTION,
];

/// Sistemas reclamados por scripts neste mundo.
#[derive(Debug, Default, Resource, Reflect)]
#[reflect(Resource)]
pub struct ScriptSystemOwners(pub HashSet<String>);

impl ScriptSystemOwners {
    /// O sistema `name` está reclamado por um script?
    pub fn owns(&self, name: &str) -> bool {
        self.0.contains(name)
    }
}

/// Avisa 1× por nome quando um script reclama um sistema que já não tem
/// handler nativo (ver a doc do módulo). O `viber.own_system` continua a
/// aceitar os nomes para os mundos existentes não falharem no load, mas a
/// posse é um no-op — o aviso é o que resta da remoção.
pub fn warn_dead_system_claims(
    owners: Res<ScriptSystemOwners>,
    mut warned: Local<HashSet<String>>,
) {
    for name in owners.0.iter() {
        if name != SYSTEM_LOCOMOTION && warned.insert(name.clone()) {
            warn!(
                "viber.own_system(\"{name}\"): sistema sem handler nativo desde que a lógica passou para Lua — posse ignorada"
            );
        }
    }
}
