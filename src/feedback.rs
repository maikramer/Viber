//! FX de feedback GENÉRICOS de impacto:
//!
//! - **Dano flutuante**: pool de textos UI world-anchored (projecção da
//!   câmara) que sobem e desvanecem — `-25` no alvo, dourado para loot — com
//!   pop de escala com overshoot nos primeiros ~0.1 s.
//! - **Hit flash**: acende os materiais da entidade atingida em branco-quente
//!   e reverte no fim (clones por instância; nada de pintar todos os lobos).
//! - **Vinheta de HP baixo**: borda vermelha pulsante abaixo de 30 % de HP.
//!
//! Dano ao herói, i-frames, morte/respawn, veneno e alvo de combate são
//! regras do JOGO (Lua no Vael). A engine só desenha o feedback que o script
//! pede (`viber.damage_number`, `viber.hit_flash`, `viber.shake`, …).

use bevy::camera::Camera3d;
use bevy::prelude::*;

use crate::luau::ScriptToast;
use crate::player::Player;
use crate::vitals::Health;

/// Vida útil de um número de dano (s).
const NUMBER_LIFETIME: f32 = 0.9;
/// Tamanho do pool de números de dano (slots reutilizados).
const NUMBER_POOL: usize = 14;
/// Vida do flash no inimigo atingido (s). Um glint no frame do toque.
pub const HIT_FLASH_SECS: f32 = 0.16;
/// Pico do emissive do flash. Alto o bastante para ler, baixo o bastante
/// para a criatura não virar uma silhueta branca.
const HIT_FLASH_EMISSIVE: f32 = 1.6;
/// Teto de materiais clonados por flash (GLBs de personagem têm < 20).
/// `pub(crate)`: o fade de corpses (`combat.rs`) partilha o mesmo cap.
pub(crate) const MAX_FLASH_MATS: usize = 24;

// ── eventos ─────────────────────────────────────────────────────────────

/// Número flutuante a mostrar no mundo (consumido pelo pool de UI).
#[derive(Debug, Clone, bevy::ecs::message::Message)]
pub struct DamageNumberEvent {
    pub position: Vec3,
    pub text: String,
    pub color: Color,
}

/// Aggro-chain (loop 6): o herói acertou uma criatura nesta posição —
/// aliados num raio de 15 m recebem `on_player_attack(px, pz)` nos scripts.
#[derive(Debug, Clone, Copy, bevy::ecs::message::Message)]
pub struct AttackAlert {
    pub position: Vec3,
}

// ── componentes / recursos ──────────────────────────────────────────────

/// Slot do pool de números de dano.
#[derive(Debug, Clone, Component)]
struct DamageNumberSlot {
    world_pos: Vec3,
    age: f32,
    active: bool,
    /// Tamanho de fonte atribuído à ativação ([`number_font_size`]) — a base
    /// do pop de escala ([`damage_number_scale`]).
    base_font_size: f32,
}

/// Flash de impacto no inimigo: enquanto `timer > 0`, a subárvore do modelo
/// acende em branco-quente (`HIT_FLASH_EMISSIVE` no pico, queda quadrática).
/// Inserido pelas vias de dano do combate (melee, fireball).
#[derive(Debug, Clone, Component)]
pub struct HitFlash {
    pub timer: f32,
}

/// Materiais CLONADOS que esta entidade possui durante o flash. Os GLBs das
/// criaturas partilham materiais entre instâncias do mesmo spawner — mutar o
/// asset original acenderia TODOS os lobos quando um apanha. O clone é
/// trocado no 1.º frame do flash e fica (visualmente idêntico, emissive
/// apagado no fim) — sem reversão de handles.
#[derive(Component)]
/// Materiais clonados para o hit-flash de UM inimigo: `(nó, original, clone)`.
/// O clone é necessário (os assets são partilhados entre instâncias — mutar o
/// original acenderia TODOS os lobos), mas o par `(nó, original)` existe para
/// o fim do flash REVERTER a troca e REMOVER o clone do asset store: o Bevy
/// não faz GC de assets, portanto deixá-los ficar (o comportamento anterior)
/// era um leak de até [`MAX_FLASH_MATS`] materiais POR GOLPE, para sempre.
struct FlashMaterials(Vec<(Entity, Handle<StandardMaterial>, Handle<StandardMaterial>)>);

/// Intensidade do flash no instante com `timer` restante — queda quadrática:
/// pop imediato, desvanecimento rápido (o golpe tem de LER no frame do toque).
pub fn hit_flash_intensity(timer: f32) -> f32 {
    let t = (timer / HIT_FLASH_SECS).clamp(0.0, 1.0);
    t * t
}

/// Tamanho da fonte de um número de dano. O crítico é um pouco maior; o
/// texto em si fica o número, sem letreiro.
pub fn number_font_size(text: &str) -> f32 {
    if text.contains("CRIT") {
        32.0
    } else if text.contains('!') || text.contains("x2") {
        28.0
    } else {
        24.0
    }
}

// ── pop de escala dos números de dano ───────────────────────────────────

/// Overshoot do pop de escala (1.25× do tamanho base).
pub const NUMBER_POP_PEAK: f32 = 1.25;
/// Idade (s) em que o pop atinge o pico do overshoot.
pub const NUMBER_POP_PEAK_AT: f32 = 0.04;
/// Idade (s) em que o pop assenta em 1.0.
pub const NUMBER_POP_SETTLE_AT: f32 = 0.10;

/// smootherstep (quíntica, C²): as duas fases do pop não têm vincos.
fn smootherstep(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

/// Pop de escala de um número de dano com `age` segundos: sobe suave até
/// [`NUMBER_POP_PEAK`]× aos [`NUMBER_POP_PEAK_AT`] s e assenta em 1.0 aos
/// [`NUMBER_POP_SETTLE_AT`] s; depois disso é 1.0 (o fade é da alpha, já
/// existente). Puro para testes.
pub fn damage_number_scale(age: f32) -> f32 {
    if age <= 0.0 {
        return 1.0;
    }
    if age <= NUMBER_POP_PEAK_AT {
        let t = age / NUMBER_POP_PEAK_AT;
        return 1.0 + (NUMBER_POP_PEAK - 1.0) * smootherstep(t);
    }
    if age <= NUMBER_POP_SETTLE_AT {
        let t = (age - NUMBER_POP_PEAK_AT) / (NUMBER_POP_SETTLE_AT - NUMBER_POP_PEAK_AT);
        return NUMBER_POP_PEAK - (NUMBER_POP_PEAK - 1.0) * smootherstep(t);
    }
    1.0
}

// ── lógica pura (testada) ───────────────────────────────────────────────

pub struct FeedbackPlugin;

impl Plugin for FeedbackPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<ScriptToast>()
            .add_message::<DamageNumberEvent>()
            .add_message::<AttackAlert>()
            .add_message::<crate::ambient::SfxEvent>()
            // O hit-flash clona StandardMaterials — apps mínimas precisam do
            // registo (bare `Assets`, sem AssetServer).
            .init_resource::<Assets<StandardMaterial>>()
            .add_systems(
                Startup,
                (spawn_low_hp_vignette, spawn_damage_number_pool),
            )
            .add_systems(
                Update,
                (low_hp_vignette_system, damage_numbers_system, hit_flash_system),
            );
    }
}

// ── spawn de UI ─────────────────────────────────────────────────────────

/// Vinheta de HP baixo: acorda quando a fração de HP passa abaixo de
/// [`LOW_HP_FRACTION`].
#[derive(Component)]
struct LowHpVignette;

fn spawn_low_hp_vignette(mut commands: Commands) {
    commands.spawn((
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(0.0),
            left: Val::Px(0.0),
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            border: UiRect::all(Val::Px(72.0)),
            ..Default::default()
        },
        BackgroundColor(Color::NONE),
        BorderColor::all(Color::NONE),
        Name::new("fx:lowhp-vignette"),
        LowHpVignette,
    ));
}

/// Pool de números de dano — sistema EXCLUSIVO porque a fonte (Cinzel, a
/// mesma do HUD) vive em [`crate::hud::assets::HudAssets`], cujo `get` pede
/// `&mut World` (lazy-init do recurso + assets). Apps mínimas não têm os
/// registos do AssetPlugin: init aqui, idempotente.
fn spawn_damage_number_pool(world: &mut World) {
    if world.get_resource::<Assets<Font>>().is_none() {
        world.init_resource::<Assets<Font>>();
    }
    if world.get_resource::<Assets<Image>>().is_none() {
        world.init_resource::<Assets<Image>>();
    }
    let font = crate::hud::assets::HudAssets::get(world).font;
    for i in 0..NUMBER_POOL {
        world.spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(0.0),
                top: Val::Px(0.0),
                ..Default::default()
            },
            Text::new(""),
            TextColor(Color::NONE),
            TextFont {
                font: font.clone().into(),
                font_size: 30.0.into(),
                ..Default::default()
            },
            Name::new(format!("fx:dmg-{i}")),
            Visibility::Hidden,
            DamageNumberSlot {
                world_pos: Vec3::ZERO,
                age: 0.0,
                active: false,
                base_font_size: 30.0,
            },
        ));
    }
}

// ── sistemas ────────────────────────────────────────────────────────────

/// Fração de HP abaixo da qual a vinheta persistente pulsa.
pub const LOW_HP_FRACTION: f32 = 0.30;
/// Frequência do pulso (Hz).
pub const LOW_HP_PULSE_HZ: f32 = 1.1;
/// Fade in/out da vinheta (s) — a vinheta não pisca a entrar nem a sair.
pub const LOW_HP_FADE_SECS: f32 = 0.5;

/// Fade suavizado da vinheta de HP baixo: move `current` para `target`
/// (0/1) linearmente ao longo de [`LOW_HP_FADE_SECS`]. Puro para testes.
pub fn low_hp_fade(current: f32, target: f32, dt: f32) -> f32 {
    let step = (dt / LOW_HP_FADE_SECS).max(0.0);
    if target > current {
        (current + step).min(target)
    } else {
        (current - step).max(target)
    }
}

/// Alpha da vinheta de HP baixo: fade suavizado × pulso senoidal
/// ([`LOW_HP_PULSE_HZ`]) × profundidade (∝ (0.30 − fração)/0.30, clampada).
/// Frações ≥ 0.30 devolvem 0. Puro para testes.
pub fn low_hp_alpha(fraction: f32, fade: f32, t: f32) -> f32 {
    let depth = ((LOW_HP_FRACTION - fraction) / LOW_HP_FRACTION).clamp(0.0, 1.0);
    let pulse = 0.5 + 0.5 * (std::f32::consts::TAU * LOW_HP_PULSE_HZ * t).sin();
    (fade * pulse * depth).clamp(0.0, 1.0)
}

/// Vinheta de HP baixo: borda vermelho-escura pulsante enquanto a fração de
/// HP do herói está abaixo de [`LOW_HP_FRACTION`].
#[allow(clippy::type_complexity)]
fn low_hp_vignette_system(
    time: Res<Time>,
    players: Query<&Health, With<Player>>,
    mut fade: Local<f32>,
    mut q_vignette: Query<(&mut BackgroundColor, &mut BorderColor), With<LowHpVignette>>,
) {
    let Ok((mut bg, mut border)) = q_vignette.single_mut() else {
        return;
    };
    let fraction = players
        .single()
        .ok()
        .map(|hp| {
            if hp.max > 0.0 {
                hp.current / hp.max
            } else {
                0.0
            }
        })
        // Sem herói (apps mínimas): a vinheta esvazia.
        .unwrap_or(1.0);
    let target = if fraction < LOW_HP_FRACTION { 1.0 } else { 0.0 };
    *fade = low_hp_fade(*fade, target, time.delta_secs());
    let alpha = low_hp_alpha(fraction, *fade, time.elapsed_secs());
    bg.0 = Color::srgba(0.45, 0.02, 0.02, 0.16 * alpha);
    *border = BorderColor::all(Color::srgba(0.55, 0.03, 0.03, 0.55 * alpha));
}

/// Entidades da subárvore (raiz incluída) — os GLBs spawnam os meshes como
/// filhos; o flash tem de os alcançar todos. `pub(crate)`: o fade de corpses
/// (`combat.rs`) percorre a mesma subárvore.
pub(crate) fn collect_subtree(root: Entity, children: &Query<&Children>, out: &mut Vec<Entity>) {
    out.push(root);
    for child in children.get(root).into_iter().flatten() {
        collect_subtree(*child, children, out);
    }
}

/// Hit-flash do inimigo: no 1.º frame clona os materiais da subárvore (os
/// assets são partilhados entre instâncias — mutar o original acenderia
/// TODOS os lobos quando um apanha), troca os handles e acende-os em
/// branco-quente; nos seguintes, anima o emissive até apagar. No fim, REVERTE
/// a troca e remove os clones — restaurar o original é até mais fiel do que
/// o comportamento antigo (deixava um clone com emissive a PRETO, que apagava
/// o brilho de materiais emissivos originais) e fecha o leak.
#[allow(clippy::type_complexity)]
fn hit_flash_system(
    time: Res<Time>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut flashed: Query<(Entity, &mut HitFlash, Option<&mut FlashMaterials>), Without<Player>>,
    mut mesh_materials: Query<&mut MeshMaterial3d<StandardMaterial>>,
    children: Query<&Children>,
    mut commands: Commands,
) {
    let dt = time.delta_secs();
    for (entity, mut flash, mut owned) in &mut flashed {
        flash.timer -= dt;
        if flash.timer <= 0.0 {
            if let Some(owned) = owned.as_mut() {
                for (node, original, clone) in owned.0.iter() {
                    // Só reverte se o handle do nó ainda FOR o nosso clone:
                    // se entretanto outra coisa o trocou (o cadáver clona de
                    // novo para o fade), o clone já não está referenciado e
                    // removê-lo é seguro de qualquer forma.
                    if let Ok(mut slot) = mesh_materials.get_mut(*node)
                        && slot.0 == *clone
                    {
                        slot.0 = original.clone();
                    }
                    materials.remove(clone);
                }
            }
            commands
                .entity(entity)
                .remove::<(HitFlash, FlashMaterials)>();
            continue;
        }
        let intensity = hit_flash_intensity(flash.timer);
        let emissive = LinearRgba::rgb(
            HIT_FLASH_EMISSIVE * intensity,
            HIT_FLASH_EMISSIVE * 0.88 * intensity,
            HIT_FLASH_EMISSIVE * 0.68 * intensity,
        );
        if let Some(owned) = owned.as_mut() {
            for (_, _, clone) in owned.0.iter() {
                if let Some(mut material) = materials.get_mut(clone) {
                    material.emissive = emissive;
                }
            }
        } else {
            // Primeiro frame do flash: clona e troca (uma vez por flash).
            let mut subtree = Vec::new();
            collect_subtree(entity, &children, &mut subtree);
            let mut cloned: Vec<(Entity, Handle<StandardMaterial>, Handle<StandardMaterial>)> =
                Vec::new();
            for node in subtree {
                if cloned.len() >= MAX_FLASH_MATS {
                    break;
                }
                let Ok(mut slot) = mesh_materials.get_mut(node) else {
                    continue;
                };
                let Some(original) = materials.get(&slot.0) else {
                    continue;
                };
                let mut copy = original.clone();
                copy.emissive = emissive;
                let handle = materials.add(copy);
                let original = std::mem::replace(&mut slot.0, handle.clone());
                cloned.push((node, original, handle));
            }
            commands.entity(entity).insert(FlashMaterials(cloned));
        }
    }
}

/// Números de dano: projecta no ecrã, sobe e desvanece. O tamanho da fonte
/// acompanha o tipo de golpe ([`number_font_size`]) — crítico/remate dominam
/// — e os primeiros ~0.1 s têm um pop de escala com overshoot
/// ([`damage_number_scale`]).
#[allow(clippy::type_complexity)]
fn damage_numbers_system(
    time: Res<Time>,
    mut incoming: MessageReader<DamageNumberEvent>,
    mut slots: Query<(
        &mut DamageNumberSlot,
        &mut Node,
        &mut Text,
        &mut TextColor,
        &mut TextFont,
        &mut Visibility,
    )>,
    camera: Query<(&Camera, &GlobalTransform), With<Camera3d>>,
) {
    for event in incoming.read() {
        let font_size = number_font_size(&event.text);
        for (mut slot, _node, mut text, mut color, mut font, mut visibility) in &mut slots {
            if slot.active {
                continue;
            }
            slot.active = true;
            slot.world_pos = event.position;
            slot.age = 0.0;
            slot.base_font_size = font_size;
            *text = Text::new(event.text.clone());
            color.0 = event.color;
            font.font_size = font_size.into();
            *visibility = Visibility::Inherited;
            info!(target: "viber::feedback", "número '{}' ativado em {:#?}", event.text, event.position);
            break;
        }
    }
    let Ok((camera, camera_transform)) = camera.single() else {
        return;
    };
    let dt = time.delta_secs();
    for (mut slot, mut node, _text, mut color, mut font, mut visibility) in &mut slots {
        if !slot.active {
            continue;
        }
        slot.age += dt;
        if slot.age >= NUMBER_LIFETIME {
            slot.active = false;
            *visibility = Visibility::Hidden;
            color.0.set_alpha(0.0);
            continue;
        }
        let projected = camera.world_to_viewport(
            camera_transform,
            slot.world_pos + Vec3::Y * (slot.age * 0.6),
        );
        let Ok(screen) = projected else {
            // pode acontecer em apps sem viewport (headless/tests)
            debug!(target: "viber::feedback", "projeção falhou: {projected:?}");
            *visibility = Visibility::Hidden;
            continue;
        };
        if slot.age - dt < 0.05 {
            info!(target: "viber::feedback", "número projetado em {screen:?}");
        }
        // world_to_viewport devolve o canto superior-esquerdo em px lógicos;
        // o texto fica um pouco acima do ponto e centrado a olho.
        node.left = Val::Px(screen.x - 18.0);
        node.top = Val::Px(screen.y - 34.0);
        let fade = (1.0 - slot.age / NUMBER_LIFETIME).clamp(0.0, 1.0);
        color.0.set_alpha(fade);
        // Pop de escala: overshoot 1.25× nos primeiros 40 ms, assente a 1.0
        // a partir de ~100 ms (escala no tamanho da fonte — bevy_ui não tem
        // transform de escala de texto fiável).
        font.font_size = (slot.base_font_size * damage_number_scale(slot.age)).into();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_damage_number_slot_activates_and_projects() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(FeedbackPlugin);
        app.world_mut().spawn((
            Camera3d::default(),
            Camera::default(),
            Projection::default(),
            GlobalTransform::from(Transform::from_xyz(0.0, 2.0, 6.0)),
        ));
        app.update(); // Startup: pool + vignette
        app.world_mut().write_message(DamageNumberEvent {
            position: Vec3::new(0.0, 1.5, 0.0),
            text: "-10".into(),
            color: Color::srgb(1.0, 0.3, 0.2),
        });
        app.update(); // leitura do evento + projeção

        let world = app.world_mut();
        let mut q = world.query::<(&DamageNumberSlot, &Visibility, &Node, &Text)>();
        let mut active = 0;
        for (slot, _visibility, _node, text) in q.iter(world) {
            if slot.active {
                active += 1;
                assert_eq!(text.0, "-10");
            }
        }
        assert_eq!(active, 1, "exatamente um slot ativado pelo evento");
        // Nota: a projeção (world_to_viewport) precisa de viewport real —
        // headless ela esconde o slot; o render é verificado in-game via
        // screenshots da bridge (validação do loop).
    }

    #[test]
    fn test_number_font_size_ranks_impact() {
        assert_eq!(number_font_size("-25"), 24.0);
        assert_eq!(number_font_size("-50 x2"), 28.0, "backstab");
        assert_eq!(number_font_size("EXECUTADO!"), 28.0);
        assert_eq!(number_font_size("GOLPE FINAL!"), 28.0);
        assert_eq!(number_font_size("-63 CRIT!"), 32.0, "crítico domina");
    }

    #[test]
    fn test_hit_flash_intensity_quadratic_falloff() {
        assert_eq!(hit_flash_intensity(HIT_FLASH_SECS), 1.0, "pico");
        let mid = hit_flash_intensity(HIT_FLASH_SECS * 0.5);
        assert!((mid - 0.25).abs() < 1e-5, "quadrática: {mid}");
        assert_eq!(hit_flash_intensity(0.0), 0.0);
        assert_eq!(hit_flash_intensity(-1.0), 0.0, "clamp em baixo");
        assert_eq!(
            hit_flash_intensity(HIT_FLASH_SECS * 4.0),
            1.0,
            "clamp em cima"
        );
    }

    // ── pop de escala dos números de dano ────────────────────────────────

    #[test]
    fn test_damage_number_scale_overshoots_and_settles() {
        // Arranca em 1.0, pico de 1.25× exato aos 40 ms, assente em 1.0.
        assert!((damage_number_scale(0.0) - 1.0).abs() < 1e-5);
        assert!(
            (damage_number_scale(-1.0) - 1.0).abs() < 1e-5,
            "idade inválida = 1.0"
        );
        assert!((damage_number_scale(NUMBER_POP_PEAK_AT) - NUMBER_POP_PEAK).abs() < 1e-4);
        assert!((damage_number_scale(NUMBER_POP_SETTLE_AT) - 1.0).abs() < 1e-4);
        assert!(
            (damage_number_scale(0.5) - 1.0).abs() < 1e-5,
            "passado o pop"
        );
        // Dentro da subida: entre 1.0 e o pico, monotónico crescente.
        let quarter = damage_number_scale(NUMBER_POP_PEAK_AT * 0.25);
        assert!(quarter > 1.0 && quarter < NUMBER_POP_PEAK, "{quarter}");
        let half = damage_number_scale(NUMBER_POP_PEAK_AT * 0.5);
        assert!(half > quarter && half < NUMBER_POP_PEAK, "{half}");
        // Na descida: entre o pico e 1.0, monotónico decrescente.
        let down = damage_number_scale((NUMBER_POP_PEAK_AT + NUMBER_POP_SETTLE_AT) * 0.5);
        assert!(down > 1.0 && down < NUMBER_POP_PEAK, "{down}");
    }

    // ── vinheta de HP baixo ──────────────────────────────────────────────

    #[test]
    fn test_low_hp_alpha_depth_and_pulse() {
        let t = 1.23; // instante qualquer
        // HP cheio: nada, mesmo com fade a 1.
        assert_eq!(low_hp_alpha(1.0, 1.0, t), 0.0);
        // Exatamente no limiar: profundidade 0 → invisível.
        assert_eq!(low_hp_alpha(LOW_HP_FRACTION, 1.0, t), 0.0);
        // A zero de HP, no pico do pulso: alpha = fade.
        let peak_t = 0.25 / LOW_HP_PULSE_HZ; // sin = 1 → pulso = 1
        assert!((low_hp_alpha(0.0, 1.0, peak_t) - 1.0).abs() < 1e-4);
        // Metade da profundidade: metade do alpha (mesmo instante).
        let half = low_hp_alpha(LOW_HP_FRACTION * 0.5, 1.0, peak_t);
        assert!((half - 0.5).abs() < 1e-4, "{half}");
        // Pulso é senoidal: há instantes com alpha 0 mesmo a HP 0.
        let trough_t = 0.75 / LOW_HP_PULSE_HZ; // sin = -1 → pulso = 0
        assert!(low_hp_alpha(0.0, 1.0, trough_t) < 1e-4);
        // Fade a 0: nada.
        assert_eq!(low_hp_alpha(0.0, 0.0, t), 0.0);
    }

    #[test]
    fn test_low_hp_fade_reaches_target_in_half_a_second() {
        // Sobe 0→1 em LOW_HP_FADE_SECS.
        let mut f = 0.0;
        let dt = 1.0 / 60.0;
        for _ in 0..60 {
            f = low_hp_fade(f, 1.0, dt);
        }
        assert!((f - 1.0).abs() < 1e-4, "fade in completo: {f}");
        // Desce 1→0 no mesmo tempo.
        for _ in 0..60 {
            f = low_hp_fade(f, 0.0, dt);
        }
        assert!(f.abs() < 1e-4, "fade out completo: {f}");
        // dt 0 não mexe; clamp nos extremos.
        assert_eq!(low_hp_fade(0.4, 1.0, 0.0), 0.4);
        assert_eq!(low_hp_fade(0.4, 1.0, 10.0), 1.0, "clamp no alvo");
    }

    // ── anel de alvo ─────────────────────────────────────────────────────

    #[test]
    fn test_hit_flash_clones_material_and_restores() {
        use bevy::mesh::PrimitiveTopology;
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(FeedbackPlugin);
        app.init_resource::<Assets<bevy::mesh::Mesh>>();
        app.update(); // Startup: pool + vignette + Assets registado

        let world = app.world_mut();
        let mesh = world
            .resource_mut::<Assets<bevy::mesh::Mesh>>()
            .add(bevy::mesh::Mesh::new(
                PrimitiveTopology::TriangleList,
                bevy::asset::RenderAssetUsages::MAIN_WORLD,
            ));
        let original = world
            .resource_mut::<Assets<StandardMaterial>>()
            .add(StandardMaterial {
                base_color: Color::srgb(0.8, 0.2, 0.1),
                ..Default::default()
            });
        // Linha de base DEPOIS de criar o material do teste: o FeedbackPlugin
        // também tem materiais próprios (vinheta, …), portanto o que se mede
        // é o DELTA do flash, não a contagem absoluta do store.
        let baseline = world.resource::<Assets<StandardMaterial>>().len();
        let enemy = world
            .spawn((Mesh3d(mesh), MeshMaterial3d(original.clone())))
            .id();
        world.entity_mut(enemy).insert(HitFlash {
            timer: HIT_FLASH_SECS,
        });
        // Frame do 1.º flash: clona o material, acende, original INTACTO.
        app.update();
        let world = app.world_mut();
        let original_after = world
            .resource::<Assets<StandardMaterial>>()
            .get(&original)
            .unwrap()
            .clone();
        assert_eq!(
            original_after.emissive,
            LinearRgba::BLACK,
            "asset partilhado não pode acender (todas as instâncias brilhariam)"
        );
        // O CLONE (agora no slot da entidade) está aceso no pico.
        {
            let mut q = world.query::<&MeshMaterial3d<StandardMaterial>>();
            let handle = q.get(world, enemy).unwrap().0.clone();
            let clone = world
                .resource::<Assets<StandardMaterial>>()
                .get(&handle)
                .unwrap();
            assert!(
                clone.emissive.red > HIT_FLASH_EMISSIVE * 0.9,
                "flash aceso no clone: {:#?}",
                clone.emissive
            );
            assert!(handle != original, "slot aponta para o clone");
        }
        // Força a expiração (o TimePlugin do MinimalPlugins usa deltas reais
        // ~0 — escrever o timer a 0 é a via determinística no teste).
        world.entity_mut(enemy).insert(HitFlash { timer: 0.0 });
        app.update(); // remove + emissive a preto no frame de expiração

        let world = app.world_mut();
        assert!(
            world.get::<HitFlash>(enemy).is_none(),
            "flash removido no fim"
        );
        assert!(
            world.get::<FlashMaterials>(enemy).is_none(),
            "clone desanexado"
        );
        let mut q = world.query::<&MeshMaterial3d<StandardMaterial>>();
        let current = q.get(world, enemy).unwrap().0.clone();
        assert_eq!(
            current, original,
            "o slot VOLTA ao material original — o clone não fica (leak fechado)"
        );
        assert!(
            !world
                .resource::<Assets<StandardMaterial>>()
                .contains(&current)
                || current == original,
            "sem clones órfãos no store"
        );
        assert_eq!(
            world.resource::<Assets<StandardMaterial>>().len(),
            baseline,
            "o clone foi removido — o store volta à linha de base do arranque"
        );
    }

    /// O hit-flash FECHA A CONTA: no fim do flash o nó volta ao material
    /// ORIGINAL e o clone sai do asset store. O comportamento anterior deixava
    /// o clone residente para sempre (o Bevy não faz GC de assets) — até
    /// `MAX_FLASH_MATS` materiais POR GOLPE a acumular numa sessão.
    #[test]
    fn test_hit_flash_remove_os_clones_no_fim() {
        let mut app = App::new();
        app.init_resource::<Time>()
            .init_resource::<Assets<StandardMaterial>>()
            .add_systems(Update, hit_flash_system);

        let (enemy, child, original) = {
            let world = app.world_mut();
            let original = world
                .resource_mut::<Assets<StandardMaterial>>()
                .add(StandardMaterial::default());
            let enemy = world
                .spawn(HitFlash {
                    timer: HIT_FLASH_SECS,
                })
                .id();
            let child = world
                .spawn((MeshMaterial3d(original.clone()), ChildOf(enemy)))
                .id();
            (enemy, child, original)
        };

        app.update(); // 1.º frame: clona e troca
        assert_eq!(
            app.world().resource::<Assets<StandardMaterial>>().len(),
            2,
            "o clone existe durante o flash"
        );

        for mut flash in app
            .world_mut()
            .query::<&mut HitFlash>()
            .iter_mut(app.world_mut())
        {
            flash.timer = 0.0;
        }
        app.update(); // fim: reverte e remove

        let world = app.world();
        assert_eq!(
            world.resource::<Assets<StandardMaterial>>().len(),
            1,
            "o clone foi removido do asset store"
        );
        assert_eq!(
            world
                .get::<MeshMaterial3d<StandardMaterial>>(child)
                .expect("o nó sobrevive ao flash")
                .0,
            original,
            "o nó voltou ao material ORIGINAL"
        );
        assert!(
            world.get_entity(enemy).is_ok(),
            "o inimigo sobrevive — só o flash sai"
        );
    }
}
