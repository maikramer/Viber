//! Fills [`UiData`] once per frame from the world's own resources and from
//! the interaction/zone systems.
//!
//! This is the single place the declarative UI touches engine state. Keeping
//! it in one system means a HUD element never queries the world itself: worlds
//! and scripts only ever see the named bindings, which is what makes the
//! interface re-authorable without recompiling the engine.

use bevy::prelude::*;

use super::bind::UiData;
use super::runtime::{UiBind, UiTag};

/// Newest toast plus its remaining lifetime, so `toast.active` can fade.
#[derive(Debug, Default, Resource, Reflect)]
#[reflect(Resource)]
pub struct UiToast {
    pub text: String,
    pub time: f32,
}

/// Seconds a toast stays bound before `toast.active` goes false.
pub const TOAST_LIFETIME: f32 = 3.5;

/// Formats an in-world minute-of-day as `HH:MM`.
pub fn clock_text(minute_of_day: f32) -> String {
    let minutes = minute_of_day.rem_euclid(1440.0) as u32;
    format!("{:02}:{:02}", minutes / 60, minutes % 60)
}

/// Keeps the toast resource fed from the script/gameplay toast channel.
///
/// Avisos de zona ("Entraste em: …", "De volta ao vale.") NÃO chegam aqui: o
/// deles é o cartão de descoberta (`zone.name`/`zone.active`), que tem ritmo,
/// tamanho e SFX próprios — o mesmo texto em duas camadas era o ruído que
/// entulhava o HUD antigo. Filtrado do lado display, conforme o contrato.
pub fn collect_ui_toasts(
    mut toast: ResMut<UiToast>,
    mut incoming: bevy::ecs::message::MessageReader<crate::luau::ScriptToast>,
    time: Res<Time>,
) {
    for message in incoming.read() {
        if crate::menus::is_zone_notice(&message.0) {
            continue;
        }
        toast.text = message.0.clone();
        toast.time = TOAST_LIFETIME;
    }
    if toast.time > 0.0 {
        toast.time = (toast.time - time.delta_secs()).max(0.0);
    }
}

/// Retira a camada legacy de toasts em Rust (`menus::spawn_toast_container`,
/// nome `"ui:toasts"`) — pilha de até 3 pílulas que corria EM PARALELO com o
/// toast declarativo deste módulo. Só retira quando existe um texto
/// declarativo com `bind="toast"`: mundos sem consumidor mantêm o fallback.
///
/// Corre em `Update` (a ordem dentro de `Startup` não é garantida entre
/// plugins) e desiste no primeiro sucesso. Sem consumidor ainda tenta nos
/// frames seguintes, para permitir que a árvore declarativa nasça mais tarde.
/// Se `menus.rs` renomear o contentor, o fallback é ver duas camadas — nunca zero.
#[allow(clippy::type_complexity)]
pub fn retire_legacy_toast_layer(
    mut commands: Commands,
    mut done: Local<bool>,
    names: Query<(Entity, &Name)>,
    consumers: Query<&UiBind, (With<Text>, With<UiTag>)>,
) {
    if *done || !consumers.iter().any(|bind| bind.0 == "toast") {
        return;
    }
    for (entity, name) in &names {
        if name.as_str() == "ui:toasts" {
            commands.entity(entity).despawn();
            *done = true;
            debug!("ui: camada legacy de toasts retirada — o toast é o declarativo");
            return;
        }
    }
}

/// Publishes the nearest interactable into [`UiPrompt`], which is what the
/// HUD's key-cap prompt binds to.
///
/// The label is the verb, not a sentence: the prompt is two glyphs and a word
/// floating over the world, and anything longer reads as a tooltip.
pub fn collect_ui_prompt(
    mut prompt: ResMut<UiPrompt>,
    time: Res<Time>,
    players: Query<&GlobalTransform, With<crate::player::Player>>,
    targets: Query<
        (&GlobalTransform, &crate::luau::ScriptInteraction),
        Without<crate::player::Player>,
    >,
) {
    let Ok(player) = players.single() else {
        prompt.clear();
        return;
    };
    let origin = player.translation();
    // Nearest wins, across both kinds of interactable — standing between a
    // merchant and a berry bush should offer the closer one, not the first the
    // query happens to yield.
    let mut best: Option<(f32, String, String)> = None;
    let mut consider = |distance: f32, key: String, label: String| {
        if best.as_ref().is_none_or(|(d, _, _)| distance < *d) {
            best = Some((distance, key, label));
        }
    };
    for (transform, interaction) in &targets {
        let distance = transform.translation().distance(origin);
        if distance <= interaction.range {
            // The script authored the verb; fall back to a generic one only
            // when it left the label empty.
            let label = if interaction.label.trim().is_empty() {
                "usar".to_string()
            } else {
                interaction.label.clone()
            };
            consider(distance, key_label(interaction.key), label);
        }
    }
    match best {
        Some((_, key, label)) => {
            prompt.key = key;
            prompt.label = label;
            // Interação nativa é dona do prompt enquanto estiver ao alcance.
            prompt.script_ttl = 0.0;
        }
        None => {
            // Um prompt de script foi publicado DEPOIS deste collect (os
            // scripts correm após UiSet::Collect). Preserva-o até expirar;
            // sem TTL, um controller removido (troca de cena) deixava o
            // verbo preso no HUD.
            age_script_prompt(&mut prompt, time.delta_secs());
        }
    }
}

/// Range at which a `<DialogueNPC>` offers its prompt (m) — valor AUTORADO;
/// o efetivo é `interact::default_range()` (metade, ver `src/interact.rs`).
pub const DIALOGUE_RANGE_M: f32 = crate::interact::BASE_RANGE_M;

/// Single-glyph name for the keys an interaction can be bound to.
///
/// Cobre todas as teclas aceites por `key_code_from_str` (`src/luau.rs`:
/// `"e" "j" "f" "q" "r" "space"`) + K (loja da engine).
pub fn key_label(key: bevy::input::keyboard::KeyCode) -> String {
    use bevy::input::keyboard::KeyCode;
    match key {
        KeyCode::KeyE => "E",
        KeyCode::KeyF => "F",
        KeyCode::KeyJ => "J",
        KeyCode::KeyK => "K",
        KeyCode::KeyQ => "Q",
        KeyCode::KeyR => "R",
        KeyCode::Space => "␣",
        _ => "•",
    }
    .to_string()
}

/// Gathers everything the bindings can read.
pub fn collect_ui_data(
    mut data: ResMut<UiData>,
    day: Option<Res<crate::worldsys::DayCycleState>>,
    weather: Option<Res<crate::worldsys::WeatherState>>,
    prompt: Option<Res<UiPrompt>>,
    toast: Res<UiToast>,
) {
    // Escritas gated: `UiData` é `ResMut` e um `deref_mut` marca o recurso em
    // `Changed`; escrever a chuva incondicionalmente fazia o
    // `publish_ui_script_view` clonar o snapshot inteiro em todos os frames.
    let rain = weather.as_deref().map(|w| w.rain).unwrap_or(0.0);
    if data.weather_rain != rain {
        data.weather_rain = rain;
    }
    if let Some(day) = day.as_deref() {
        let text = clock_text(day.minute_of_day);
        if data.clock != text {
            data.clock = text;
        }
        let fraction = day.minute_of_day.rem_euclid(1440.0) / 1440.0;
        if data.day_fraction != fraction {
            data.day_fraction = fraction;
        }
    }
    match prompt.as_deref() {
        Some(prompt) => {
            if data.prompt_key != prompt.key {
                data.prompt_key = prompt.key.clone();
            }
            if data.prompt_label != prompt.label {
                data.prompt_label = prompt.label.clone();
            }
        }
        None => {
            if !data.prompt_key.is_empty() {
                data.prompt_key.clear();
            }
            if !data.prompt_label.is_empty() {
                data.prompt_label.clear();
            }
        }
    }
    if data.toast != toast.text {
        data.toast = toast.text.clone();
    }
    if data.toast_time != toast.time {
        data.toast_time = toast.time;
    }
}

// ── revelações contextuais ──────────────────────────────────────────────

/// Vida do cartão de descoberta de zona.
pub const ZONE_REVEAL_SECS: f32 = 4.5;

/// Último valor visto da zona — é o único sinal de revelação que a engine
/// conhece (vida, ouro, quests e combate são do jogo).
#[derive(Debug, Default)]
pub struct RevealMemory {
    seeded: bool,
    zone: Option<String>,
}

/// Id de zona sintético enquanto o herói está numa sala da bolsa de interior.
pub const INTERIOR_ZONE_ID: &str = "interior";

/// Nome de exposição de uma região de bioma.
///
/// Um cartão de descoberta que diz `dark-forest` é uma etiqueta de debug; o
/// que dá peso ao sítio é o nome que ele tem no mundo. A precedência é:
/// **`display-name` autoral** na região activa (XML) → título derivado do
/// próprio id, para uma zona nova não precisar nem de Rust nem de editar o
/// mundo existente. Sem id nem attr não há cartão (string vazia).
pub fn zone_display_name(display_name: Option<&str>, id: Option<&str>) -> String {
    if let Some(name) = display_name.map(str::trim).filter(|name| !name.is_empty()) {
        return name.to_string();
    }
    match id {
        None => String::new(),
        Some(id) => title_case(id),
    }
}

/// `dark-forest` → `Dark Forest`.
fn title_case(id: &str) -> String {
    id.split(['-', '_'])
        .filter(|word| !word.is_empty())
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Abre e fecha a janela do cartão de descoberta de [`UiData`].
///
/// Corre depois de `collect_ui_data`: compara a zona com o frame anterior e,
/// quando muda, publica o nome e reabre a janela. O primeiro frame é só
/// semeadura — sem isto, o HUD abria o cartão de zona no arranque, que é
/// exactamente o que estamos a tentar não ter.
pub fn collect_ui_reveals(
    mut data: ResMut<UiData>,
    mut memory: Local<RevealMemory>,
    biome: Option<Res<crate::ambient::CurrentBiome>>,
    biomes: Option<Res<crate::worldsys::BiomeRegions>>,
    interior: Option<Res<crate::worldsys::InteriorLighting>>,
    time: Res<Time>,
) {
    let dt = time.delta_secs();
    // Dentro de uma sala da bolsa a "zona" é o interior — o bioma lá fora
    // (o vale a 2,6 km) não é onde o herói está.
    let inside = interior.as_deref().is_some_and(|state| state.active);
    let zone = if inside {
        Some(INTERIOR_ZONE_ID.to_string())
    } else {
        biome.as_deref().and_then(|b| b.id.clone())
    };
    // O `display-name` da região activa vem do XML (Tinta Quente: o mundo é
    // quem nomeia os sítios); sem attr, `zone_display_name` cai no id.
    let authored = zone.as_deref().and_then(|id| {
        biomes
            .as_deref()?
            .list
            .iter()
            .find(|r| r.id == id)
            .and_then(|r| {
                let name = r.display_name.trim();
                (!name.is_empty()).then(|| name.to_string())
            })
    });

    if !memory.seeded {
        memory.seeded = true;
        memory.zone = zone.clone();
        // A zona inicial não é uma descoberta — o herói já lá está.
        data.zone_name = zone_display_name(authored.as_deref(), zone.as_deref());
        return;
    }

    if zone != memory.zone {
        memory.zone = zone.clone();
        data.zone_name = zone_display_name(authored.as_deref(), zone.as_deref());
        data.zone_reveal = ZONE_REVEAL_SECS;
    }

    tick(&mut data.zone_reveal, dt);
}

/// Desconta um frame de uma janela de revelação (nunca abaixo de zero).
fn tick(window: &mut f32, dt: f32) {
    if *window > 0.0 {
        *window = (*window - dt).max(0.0);
    }
}

/// Nearest interactable, published by the interaction system.
#[derive(Debug, Default, Resource, Reflect)]
#[reflect(Resource)]
pub struct UiPrompt {
    pub key: String,
    pub label: String,
    /// Tempo restante de um prompt publicado por script (`viber.prompt`).
    /// Os scripts correm depois de [`collect_ui_prompt`], portanto um prompt
    /// deles sobrevive ao collect seguinte enquanto estiver fresco. Uma
    /// interação nativa (`<ScriptInteraction>`) tem prioridade e zera isto.
    pub script_ttl: f32,
}

/// Quanto dura um prompt de script sem ser republicado (s). O controller
/// republica por frame; se for removido (troca de cena), o verbo expira.
pub const SCRIPT_PROMPT_TTL: f32 = 0.25;

impl UiPrompt {
    /// Publica o prompt de um script; os `on_update` seguintes renovam o TTL.
    pub fn set_script(&mut self, key: String, label: String) {
        self.key = key;
        self.label = label;
        self.script_ttl = SCRIPT_PROMPT_TTL;
    }

    /// Limpa o prompt (script cancelou ou o alvo nativo saiu de alcance).
    pub fn clear(&mut self) {
        self.key.clear();
        self.label.clear();
        self.script_ttl = 0.0;
    }
}

/// Envelhece um prompt de script. Sem TTL (prompt nativo ou já vazio), limpa;
/// com TTL, desconta o frame e limpa quando expira.
pub(crate) fn age_script_prompt(prompt: &mut UiPrompt, dt: f32) {
    if prompt.script_ttl <= 0.0 {
        prompt.clear();
        return;
    }
    prompt.script_ttl = (prompt.script_ttl - dt).max(0.0);
    if prompt.script_ttl <= 0.0 {
        prompt.clear();
    }
}

/// Reads `"2/5"` as `0.4`; anything else is 0.
pub fn progress_fraction(text: &str) -> f32 {
    let Some((done, total)) = text.split_once('/') else {
        return 0.0;
    };
    let done: f32 = done.trim().parse().unwrap_or(0.0);
    let total: f32 = total.trim().parse().unwrap_or(0.0);
    if total <= 0.0 {
        return 0.0;
    }
    (done / total).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::input::keyboard::KeyCode;

    fn toast_app() -> (App, Entity) {
        let mut app = App::new();
        app.add_systems(Update, retire_legacy_toast_layer);
        let legacy = app.world_mut().spawn(Name::new("ui:toasts")).id();
        (app, legacy)
    }

    #[test]
    fn test_legacy_toasts_stay_without_a_declarative_consumer() {
        let (mut app, legacy) = toast_app();
        app.update();
        app.update();
        assert!(app.world().get_entity(legacy).is_ok());
    }

    #[test]
    fn test_legacy_toasts_need_toast_text_on_the_same_declarative_element() {
        let (mut app, legacy) = toast_app();
        // A panel binding plus unrelated text is not a toast text consumer.
        app.world_mut()
            .spawn((UiTag("uipanel".into()), UiBind("toast".into())));
        app.world_mut()
            .spawn((UiTag("uitext".into()), Text::new("unrelated")));
        // Neither a truthiness binding nor a non-declarative text qualifies.
        app.world_mut().spawn((
            UiTag("uitext".into()),
            Text::default(),
            UiBind("toast.active".into()),
        ));
        app.world_mut()
            .spawn((Text::default(), UiBind("toast".into())));
        app.update();
        assert!(app.world().get_entity(legacy).is_ok());
    }

    #[test]
    fn test_legacy_toasts_retire_when_declarative_text_is_present() {
        let (mut app, legacy) = toast_app();
        let child = app
            .world_mut()
            .spawn((Text::new("old toast"), ChildOf(legacy)))
            .id();
        let consumer = app
            .world_mut()
            .spawn((
                super::super::runtime::UiId("notification-text".into()),
                UiTag("uitext".into()),
                Text::default(),
                UiBind("toast".into()),
            ))
            .id();
        let other = app.world_mut().spawn(Name::new("ui:other")).id();
        app.update();
        assert!(app.world().get_entity(legacy).is_err());
        assert!(app.world().get_entity(child).is_err());
        assert!(app.world().get_entity(consumer).is_ok());
        assert!(app.world().get_entity(other).is_ok());
    }

    #[test]
    fn test_legacy_toasts_retry_when_consumer_arrives_later() {
        let (mut app, legacy) = toast_app();
        app.update();
        assert!(app.world().get_entity(legacy).is_ok());
        app.world_mut().spawn((
            UiTag("uitext".into()),
            Text::default(),
            UiBind("toast".into()),
        ));
        app.update();
        assert!(app.world().get_entity(legacy).is_err());
    }

    #[test]
    fn test_key_label_is_one_glyph_and_never_empty() {
        assert_eq!(key_label(KeyCode::KeyE), "E");
        assert_eq!(key_label(KeyCode::KeyQ), "Q");
        assert_eq!(key_label(KeyCode::Space), "␣");
        // An unmapped key still prints something the cap can hold.
        let fallback = key_label(KeyCode::F7);
        assert_eq!(fallback.chars().count(), 1);
        assert!(!fallback.is_empty());
    }

    #[test]
    fn test_clock_text_wraps_the_day() {
        assert_eq!(clock_text(0.0), "00:00");
        assert_eq!(clock_text(435.0), "07:15");
        assert_eq!(clock_text(1439.0), "23:59");
        // Past midnight and before it both wrap into the day.
        assert_eq!(clock_text(1440.0), "00:00");
        assert_eq!(clock_text(-60.0), "23:00");
    }

    #[test]
    fn test_zone_display_name_derives_a_place_not_an_id() {
        // Sem zona (nem attr) não há cartão a mostrar.
        assert_eq!(zone_display_name(None, None), "");
        // O id vira título: a tabela de nomes é CONTEÚDO do mundo, não da
        // engine (Vael tem os `display-name` no XML).
        assert_eq!(zone_display_name(None, Some("dark-forest")), "Dark Forest");
        assert_eq!(
            zone_display_name(None, Some("frozen-peaks")),
            "Frozen Peaks"
        );
        // Uma região nova no XML já sai legível sem tocar em Rust.
        assert_eq!(zone_display_name(None, Some("sunken-city")), "Sunken City");
        assert_eq!(zone_display_name(None, Some("interior")), "Interior");
        assert_eq!(zone_display_name(None, Some("")), "");
    }

    #[test]
    fn test_zone_display_name_prefers_the_authored_name() {
        // display-name autoral vence o id…
        assert_eq!(
            zone_display_name(Some("Vale das Cinzas"), Some("dark-forest")),
            "Vale das Cinzas"
        );
        assert_eq!(
            zone_display_name(Some("Floresta Sombria"), Some("dark-forest")),
            "Floresta Sombria"
        );
        // …e um attr em branco não conta como nome — cai ao id derivado.
        assert_eq!(zone_display_name(Some("   "), Some("desert")), "Desert");
        assert_eq!(zone_display_name(Some("Nome"), None), "Nome");
    }

    #[test]
    fn test_progress_fraction_reads_the_quest_counter() {
        assert_eq!(progress_fraction("2/5"), 0.4);
        assert_eq!(progress_fraction("5/5"), 1.0);
        // Overshoot is clamped, and a finished-but-uncollected quest is 1.
        assert_eq!(progress_fraction("9/5"), 1.0);
        // Garbage never produces NaN or a negative bar.
        assert_eq!(progress_fraction(""), 0.0);
        assert_eq!(progress_fraction("done"), 0.0);
        assert_eq!(progress_fraction("3/0"), 0.0);
    }

    #[test]
    fn test_script_prompt_survives_collect_until_ttl_expires() {
        let mut prompt = UiPrompt::default();
        prompt.set_script("J".into(), "Cortar".into());
        // O controller republica por frame; meio TTL ainda mostra o verbo.
        age_script_prompt(&mut prompt, SCRIPT_PROMPT_TTL * 0.5);
        assert_eq!(prompt.key, "J");
        assert_eq!(prompt.label, "Cortar");
        // Sem republicação, o prompt expira (troca de cena/script removido).
        age_script_prompt(&mut prompt, SCRIPT_PROMPT_TTL);
        assert!(prompt.key.is_empty());
        assert!(prompt.label.is_empty());
        assert_eq!(prompt.script_ttl, 0.0);
    }

    #[test]
    fn test_native_prompt_never_outlives_the_target() {
        let mut prompt = UiPrompt {
            key: "E".into(),
            label: "Falar".into(),
            script_ttl: 0.0,
        };
        age_script_prompt(&mut prompt, 0.0);
        assert!(prompt.key.is_empty());
        assert!(prompt.label.is_empty());
    }
}
