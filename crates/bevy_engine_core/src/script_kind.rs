//! Nome curto de um script (`"enemies/wolf.lua"` → `"wolf"`).
//!
//! É o `kind` que viaja nos eventos `kill`/`viber.hostiles`; o jogo decide o
//! que fazer com ele. A engine só normaliza o path.

/// Tipo de criatura a partir do path do script.
pub fn script_kind(path: &str) -> String {
    path.rsplit('/')
        .next()
        .unwrap_or(path)
        .trim_end_matches(".lua")
        .to_string()
}

/// XP base de um abate no caminho NATIVO (outros presets). O Vael paga em Lua.
pub const KILL_XP: u32 = 15;

/// XP de abate por tipo (só caminho nativo).
pub fn kill_xp(kind: &str) -> u32 {
    match kind {
        "boss" => 300,
        "bog-warden" => 180,
        "sand-worm" => 160,
        "witch" => 140,
        _ => KILL_XP,
    }
}
