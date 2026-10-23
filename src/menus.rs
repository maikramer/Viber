//! UI & menus (loop 5 do port simple-rpg) — o análogo nativo do
//! TabbedModal + loja do mercador + toasts + loading screen originais:
//!
//! - **Toasts visuais**: uma fila única em baixo ao centro (3 máx.), com
//!   fade de entrada e de saída, sem sobreposição — continuam a ir para o log
//!   da bridge. Avisos de zona são filtrados: esses têm o cartão de
//!   descoberta do HUD declarativo (`zone.name`), que é outra hierarquia.
//! - **Modal [Q]** com tabs reais: Quests (ativas/feitas do [`QuestLog`]),
//!   Inventário (vault) e Ajuda (controlos + opções). ←/→ ou TAB troca de
//!   tab, [Q]/Esc fecha.
//! - **Loja [K]**: perto do `name="merchant"` — comprar poção/antídoto/
//!   bomba, vender madeira/pedra, com seleção ←/→ e confirmação [J].
//! - **Loading screen**: overlay "a forjar o mundo" que se levanta quando o
//!   jogador existe, passaram 2 s, e os albedos do terreno já carregaram
//!   (teto 8 s — os GLBs continuam a streamar).
//!
//! Enquanto o modal ou a loja estão abertos, a hotbar ([1]/[2]) não consome
//! ([`MenusOpen`]).

use bevy::prelude::*;

use crate::luau::ScriptToast;
use crate::player::Player;

/// Toasts visíveis em simultâneo. Três linhas é o que se lê de relance —
/// acima disso a fila deixa de ser informação e passa a ser ruído.
pub const TOAST_CAP: usize = 3;
/// Vida de um toast (s), fades incluídos.
pub const TOAST_LIFETIME: f32 = 3.2;
/// Fade de entrada (s).
pub const TOAST_FADE_IN: f32 = 0.22;
/// Fade de saída (s) — mais lento do que a entrada, para a linha sair de
/// cena em vez de desaparecer.
pub const TOAST_FADE_OUT: f32 = 0.55;

/// Opacidade do fundo de um toast em regime.
const TOAST_BG_ALPHA: f32 = 0.82;

/// Alpha de um toast com `remaining` segundos de vida.
///
/// Puro para os testes: o que importa é que a curva começa e acaba em zero e
/// que o patamar do meio é opaco.
pub fn toast_alpha(remaining: f32) -> f32 {
    if remaining <= 0.0 {
        return 0.0;
    }
    let elapsed = (TOAST_LIFETIME - remaining).max(0.0);
    let fade_in = if TOAST_FADE_IN > 0.0 {
        (elapsed / TOAST_FADE_IN).min(1.0)
    } else {
        1.0
    };
    let fade_out = if TOAST_FADE_OUT > 0.0 {
        (remaining / TOAST_FADE_OUT).min(1.0)
    } else {
        1.0
    };
    fade_in.min(fade_out).clamp(0.0, 1.0)
}

/// Um aviso de zona não é um toast: tem cartão próprio no HUD (`zone.name`),
/// com outra tipografia e outro tempo. Deixá-lo também na fila era a mesma
/// informação duas vezes no ecrã, uma por cima da outra.
pub fn is_zone_notice(text: &str) -> bool {
    let trimmed = text.trim();
    trimmed.starts_with("Entraste em:") || trimmed == "De volta ao vale."
}
/// Abre/fecha o estado dos menus que roubam input à hotbar.
#[derive(Debug, Clone, Resource, Default)]
pub struct MenusOpen {
    pub modal: bool,
    pub shop: bool,
    /// Painel de viagem rápida [G] (`travel::TravelMenuState.open` — o
    /// espelho vive no `travel_menu_system`). Sem este campo, com o painel
    /// aberto o player andava (W/S navegavam E moviam) e o [J] que confirma
    /// a viagem disparava o melee.
    pub travel: bool,
    /// Mapa regional [M] (`hud::map::RegionalMapState.open` — o espelho
    /// vive no `map_key_system`). O mapa é um menu: com ele aberto o herói
    /// não anda e o [L] que o fecha não dispara a guarda.
    pub map: bool,
    /// Uma cinemática está a tocar (`crate::cinematic`): o herói não anda,
    /// não ataca e a hotbar cala-se.
    pub cinematic: bool,
}

impl MenusOpen {
    pub fn any(&self) -> bool {
        self.modal || self.shop || self.travel || self.map || self.cinematic
    }
}

pub const TAB_COUNT: usize = 4;

/// Próxima tab (cíclica); pura para os testes.
pub fn next_tab(current: usize, delta: i32) -> usize {
    let count = TAB_COUNT as i32;
    ((current as i32 + delta).rem_euclid(count)) as usize
}

// ── componentes ─────────────────────────────────────────────────────────

#[derive(Component)]
struct LoadingScreen;

#[derive(Component)]
struct ToastPill {
    timer: f32,
    text: String,
}

#[derive(Component)]
struct ToastContainer;

/// Empilha o container de toasts em baixo ao centro (uma única vez).
///
/// `ColumnReverse` ancorado ao rodapé: o toast novo nasce em baixo e empurra
/// os anteriores para cima, que é como uma fila se lê. A pilha no topo-centro
/// disputava o mesmo espaço do aviso de zona e das barras do alvo.
fn spawn_toast_container(mut commands: Commands) {
    commands.spawn((
        Node {
            position_type: PositionType::Absolute,
            bottom: Val::Px(104.0),
            left: Val::Px(0.0),
            right: Val::Px(0.0),
            flex_direction: FlexDirection::ColumnReverse,
            align_items: AlignItems::Center,
            row_gap: Val::Px(5.0),
            ..Default::default()
        },
        Name::new("ui:toasts"),
        ToastContainer,
    ));
}

#[derive(Component)]
struct CampfireBanner;

// ── plugin ──────────────────────────────────────────────────────────────

pub struct MenusPlugin;

impl Plugin for MenusPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<MenusOpen>()
            .add_message::<ToastSpawned>()
            // O modal [Q] e a loja passaram para o sistema declarativo
            // (`src/ui` + `examples/simple-rpg/world/menu.xml`): tabs, listas
            // e ações vivem agora em XML/CSS/Luau. Ficam aqui os toasts, o
            // banner da fogueira e o ecrã de carregamento — nenhum deles é
            // conteúdo autoral. `MenusOpen` continua a ser a autoridade sobre
            // "um menu roubou o input"; `mirror_ui_modals_open` mantém-no
            // sincronizado com os modais declarativos.
            .add_systems(
                Startup,
                (
                    spawn_loading_screen,
                    spawn_campfire_banner,
                    spawn_toast_container,
                ),
            )
            .add_systems(
                Update,
                (
                    toast_display_system,
                    toast_fade_system,
                    // Lê o `UiModalsOpen` que o driver dos modais escreve
                    // NESTE frame (UiSet::Script).
                    mirror_ui_modals_open.after(crate::ui::UiSet::Script),
                    campfire_banner_system,
                    loading_hide_system,
                ),
            );
    }
}

/// Espelha os modais declarativos em [`MenusOpen`], que é o que a hotbar, o
/// movimento e a câmara consultam para saber se o input lhes pertence. O
/// `main.rs` pinna-o DEPOIS de `ui::modal::drive_ui_modals` (que escreve
/// `UiModalsOpen` em `UiSet::Script`) — sem ordem, o espelho lia o valor do
/// frame anterior e o input viajava dessincronizado.
pub fn mirror_ui_modals_open(
    declarative: Res<crate::ui::UiModalsOpen>,
    mut open: ResMut<MenusOpen>,
) {
    let any = declarative.any();
    if open.modal != any {
        open.modal = any;
    }
}

/// Evento interno: toast a mostrar (o HUD/log espelham).
#[derive(Debug, Clone, bevy::ecs::message::Message)]
pub struct ToastSpawned {
    pub text: String,
}

// ── toasts ──────────────────────────────────────────────────────────────

/// Lê `ScriptToast`, espelha no log e spawna a pílula visual.
///
/// Regras da fila: cap [`TOAST_CAP`], sem duplicados consecutivos (repetir a
/// mesma linha renova o tempo em vez de a empilhar), e os avisos de zona são
/// desviados para o cartão de descoberta.
fn toast_display_system(
    mut toasts: MessageReader<ScriptToast>,
    mut spawned: MessageWriter<ToastSpawned>,
    mut sfx: MessageWriter<crate::ambient::SfxEvent>,
    mut active: Query<(Entity, &mut ToastPill)>,
    container: Query<Entity, With<ToastContainer>>,
    hud: Option<Res<crate::hud::HudAssets>>,
    mut commands: Commands,
) {
    // Pílulas nascidas NESTE frame: os `commands` só aplicam no fim do
    // sistema, por isso `active` não as vê. Sem contar os pendentes, um burst
    // de mensagens no mesmo frame (loot, vários pickups) furava o cap e
    // empilhava muito mais do que TOAST_CAP linhas.
    let mut pending: Vec<String> = Vec::new();
    for toast in toasts.read() {
        info!(target: "viber::toast", "{}", toast.0);
        spawned.write(ToastSpawned {
            text: toast.0.clone(),
        });
        if is_zone_notice(&toast.0) {
            // O cartão de descoberta trata deste; o SFX de UI também não se
            // aplica (a entrada em bioma tem a sua própria camada sonora).
            continue;
        }
        sfx.write(crate::ambient::SfxEvent {
            clip: crate::ambient::SfxClip::Ui,
            position: None,
        });
        // Mesma linha outra vez (colher três bagas seguidas): renova, não
        // empilha — três cópias idênticas não dizem mais do que uma.
        if pending.iter().any(|text| *text == toast.0) {
            // Duplicado do mesmo frame: a primeira pílula já nasceu com a
            // vida cheia — renovar aqui seria empilhar duas iguais.
            continue;
        }
        if let Some((_, mut existing)) = active
            .iter_mut()
            .find(|(_, pill)| pill.text == toast.0 && pill.timer > TOAST_FADE_OUT)
        {
            existing.timer = TOAST_LIFETIME;
            continue;
        }
        // Fila cheia: o mais antigo sai já (fade curto), para o novo caber
        // sem a pilha crescer para fora do enquadramento.
        if active.iter().count() + pending.len() >= TOAST_CAP
            && let Some((oldest, _)) = active
                .iter()
                .min_by(|a, b| {
                    a.1.timer
                        .partial_cmp(&b.1.timer)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .map(|(entity, pill)| (entity, pill.timer))
            && let Ok((_, mut pill)) = active.get_mut(oldest)
        {
            pill.timer = pill.timer.min(TOAST_FADE_OUT * 0.5);
        }
        let Ok(container) = container.single() else {
            continue;
        };
        let font = hud.as_deref().map(|assets| assets.font.clone());
        commands.entity(container).with_children(|wrap_parent| {
            // ToastPill vive no NODE exterior (não no Text): o fade
            // despawna-o com o filho — antes sobrava uma caixa vazia
            // permanente por toast.
            wrap_parent
                .spawn((
                    Node {
                        padding: UiRect::axes(Val::Px(13.0), Val::Px(6.0)),
                        border: UiRect::all(Val::Px(1.0)),
                        border_radius: BorderRadius::all(Val::Px(11.0)),
                        ..Default::default()
                    },
                    BackgroundColor(Color::srgba(0.051, 0.043, 0.035, 0.0)),
                    BorderColor::all(Color::srgba(0.847, 0.706, 0.416, 0.0)),
                    Name::new("ui:toast"),
                    ToastPill {
                        timer: TOAST_LIFETIME,
                        text: toast.0.clone(),
                    },
                ))
                .with_children(|pill| {
                    let mut text_font = TextFont::from_font_size(13.0);
                    if let Some(font) = font {
                        text_font.font = bevy::text::FontSource::Handle(font);
                    }
                    pill.spawn((
                        Text::new(toast.0.clone()),
                        TextColor(Color::srgba(0.96, 0.93, 0.85, 0.0)),
                        text_font,
                        // O HUD tem de se ler sobre deserto claro e floresta
                        // escura: a sombra é o contorno barato que garante isso.
                        bevy::ui::widget::TextShadow {
                            offset: Vec2::splat(1.0),
                            color: Color::srgba(0.0, 0.0, 0.0, 0.0),
                        },
                        ToastLabel,
                    ));
                });
        });
        pending.push(toast.0.clone());
    }
}

/// O texto dentro de uma pílula (o fade tem de o alcançar).
#[derive(Component)]
struct ToastLabel;

/// Vida e fade (entrada e saída) dos toasts.
fn toast_fade_system(
    mut pills: Query<(
        Entity,
        &mut ToastPill,
        &mut BackgroundColor,
        &mut BorderColor,
        &Children,
    )>,
    mut labels: Query<(&mut TextColor, &mut bevy::ui::widget::TextShadow), With<ToastLabel>>,
    time: Res<Time>,
    mut commands: Commands,
) {
    let dt = time.delta_secs();
    for (entity, mut pill, mut bg, mut border, children) in &mut pills {
        pill.timer -= dt;
        if pill.timer <= 0.0 {
            // despawn recursivo leva o Text filho embora.
            commands.entity(entity).try_despawn();
            continue;
        }
        let alpha = toast_alpha(pill.timer);
        bg.0.set_alpha(TOAST_BG_ALPHA * alpha);
        *border = BorderColor::all(Color::srgba(0.847, 0.706, 0.416, 0.22 * alpha));
        for child in children.iter() {
            if let Ok((mut color, mut shadow)) = labels.get_mut(child) {
                color.0.set_alpha(alpha);
                shadow.color = Color::srgba(0.0, 0.0, 0.0, 0.7 * alpha);
            }
        }
    }
}

// ── modal [Q] ───────────────────────────────────────────────────────────

/// Cabeçalho das tabs (marcador > na ativa).
pub fn tab_header(active: usize) -> String {
    let names = ["Quests", "Inventário", "Skills", "Opções"];
    names
        .iter()
        .enumerate()
        .map(|(i, name)| {
            if i == active {
                format!("> {name}")
            } else {
                format!("  {name}")
            }
        })
        .collect::<Vec<_>>()
        .join("    ")
}

/// Corpo da tab Opções: volumes ao vivo, linhas com seleção e save/load.
pub fn options_body(selected: usize, volumes: (f32, f32, f32), controls: &str) -> String {
    let pct = |v: f32| format!("{}%", (v * 100.0).round() as i32);
    let row = |i: usize, label: &str, value: &str| {
        let marker = if i == selected { ">" } else { " " };
        format!("{marker} {label}: {value}")
    };
    format!(
        "{}\n{}\n{}\n{}\n{}\n\n{}\n\nControlos:\n{controls}",
        row(0, "Volume master", &pct(volumes.0)),
        row(1, "Volume música", &pct(volumes.1)),
        row(2, "Volume efeitos", &pct(volumes.2)),
        row(3, "Gravar jogo", "[J]"),
        row(4, "Carregar jogo", "[L]"),
        "↑↓ escolher · ←→ ajusta ±10% · [J]/[L] gravar/carregar",
        controls = controls,
    )
}

/// Ecrã de carregamento: o título vem do `config.yaml` do jogo (antes era o
/// nome fixo do protótipo e aparecia em qualquer jogo). `...` em ASCII: o
/// atlas de fontes da engine não tem o glifo `…` e saía um retângulo.
///
/// Não nasce no modo Edit: aí o `Time<Virtual>` está em pausa desde o
/// `Startup`, `loading_hide_system` nunca vê `elapsed > 2 s` e o véu ficava
/// VISÍVEL para sempre — no ViberEditor tapava o viewport de UI (a
/// `GameUiCamera` desenha a UI do jogo por cima da imagem).
fn spawn_loading_screen(
    mut commands: Commands,
    config: Option<Res<crate::config::GameConfig>>,
    mode: Option<Res<crate::app::RunMode>>,
) {
    if mode.as_deref() == Some(&crate::app::RunMode::Edit) {
        return;
    }
    let title = config
        .and_then(|config| config.title.clone())
        // `title: ""` é omisso (o template diz "vazio = nome do world.xml").
        .map(|title| title.trim().to_owned())
        .filter(|title| !title.is_empty())
        .map(|title| title.to_uppercase())
        .unwrap_or_else(|| "DISCORDIA".to_string());
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(0.0),
                left: Val::Px(0.0),
                right: Val::Px(0.0),
                bottom: Val::Px(0.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..Default::default()
            },
            BackgroundColor(Color::srgb(0.03, 0.03, 0.04)),
            Name::new("ui:loading"),
            LoadingScreen,
        ))
        .with_children(|root| {
            root.spawn((
                Text::new(format!("{title}\n\na forjar o mundo...")),
                TextColor(Color::srgb(0.95, 0.85, 0.6)),
                TextFont::from_font_size(26.0),
            ));
        });
}

/// Levanta o loading screen quando o jogador existe, passaram 2 s, e os
/// albedos do terreno já estão residentes (ou não há terreno de camadas).
/// Teto de 8 s — os GLBs continuam a streamar em fundo.
fn loading_hide_system(
    time: Res<Time>,
    players: Query<(), With<Player>>,
    mut loading: Query<&mut Visibility, With<LoadingScreen>>,
    server: Option<Res<AssetServer>>,
    gate: Option<Res<crate::terrain::runtime::TerrainTextureGate>>,
) {
    let textures_ready = match (&server, &gate) {
        (Some(server), Some(gate)) => gate.ready(server),
        _ => true,
    };
    let ready = players.iter().next().is_some() && time.elapsed_secs() > 2.0 && textures_ready;
    if !ready && time.elapsed_secs() < 8.0 {
        return;
    }
    for mut visibility in loading.iter_mut() {
        if *visibility != Visibility::Hidden {
            *visibility = Visibility::Hidden;
        }
    }
}

// ── banner da fogueira ──────────────────────────────────────────────────

fn spawn_campfire_banner(mut commands: Commands) {
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                bottom: Val::Px(120.0),
                left: Val::Px(0.0),
                right: Val::Px(0.0),
                justify_content: JustifyContent::Center,
                ..Default::default()
            },
            Visibility::Hidden,
            Name::new("ui:campfire"),
            CampfireBanner,
        ))
        .with_children(|wrap| {
            wrap.spawn((
                Text::new("Fogueira: [E] descansar — o calor restaura vida"),
                TextColor(Color::srgb(0.98, 0.8, 0.5)),
                TextFont::from_font_size(14.0),
            ));
        });
}

/// `haystack.contains(needle)` ASCII case-insensitive, sem alocar.
///
/// O `name.to_ascii_lowercase().contains(..)` do banner criava um `String` por
/// entidade nomeada do mundo em cada frame — milhares de `malloc`/frame no
/// `simple-rpg` (cada instância de spawner leva `Name`) só para responder a
/// uma pergunta binária.
pub(crate) fn contains_ignore_ascii_case(haystack: &str, needle: &str) -> bool {
    let needle = needle.as_bytes();
    if needle.is_empty() {
        return true;
    }
    haystack
        .as_bytes()
        .windows(needle.len())
        .any(|window| window.eq_ignore_ascii_case(needle))
}

/// Intervalo do scan de fogueiras. O banner é um prompt de HUD a 3,5 m: a
/// 0,1 s o atraso máximo é de ~6 frames e o scan (o mundo inteiro, por nome)
/// passa de 60× a 10× por segundo.
const CAMPFIRE_SCAN_INTERVAL: f32 = 0.1;

fn campfire_banner_system(
    time: Res<Time>,
    mut throttle: Local<f32>,
    players: Query<&GlobalTransform, With<Player>>,
    camps: Query<(&Name, &GlobalTransform), Without<Player>>,
    mut banner: Query<&mut Visibility, With<CampfireBanner>>,
) {
    *throttle -= time.delta_secs();
    if *throttle > 0.0 {
        return;
    }
    *throttle = CAMPFIRE_SCAN_INTERVAL;
    // Fogueira POR NOME ("campfire") — sem o marcador o banner ficava
    // praticamente sempre visível (o player está a <3,5 m de ALGUMA
    // entidade quase todo o tempo).
    let near = players.iter().next().is_some_and(|player| {
        camps.iter().any(|(name, c)| {
            contains_ignore_ascii_case(name.as_str(), "campfire")
                && c.translation().distance_squared(player.translation()) < 3.5 * 3.5
        })
    });
    for mut visibility in banner.iter_mut() {
        let wanted = if near {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
        if *visibility != wanted {
            *visibility = wanted;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_toast_alpha_opens_and_closes() {
        // Acabado de nascer: ainda invisível, a entrar.
        assert!(toast_alpha(TOAST_LIFETIME) < 0.05);
        // Meio da vida: opaco.
        assert!((toast_alpha(TOAST_LIFETIME * 0.5) - 1.0).abs() < 1e-4);
        // A sair.
        let leaving = toast_alpha(TOAST_FADE_OUT * 0.5);
        assert!(leaving > 0.0 && leaving < 0.6, "alpha={leaving}");
        assert_eq!(toast_alpha(0.0), 0.0);
        assert_eq!(toast_alpha(-1.0), 0.0);
    }

    #[test]
    fn test_zone_notices_do_not_enter_the_toast_queue() {
        assert!(is_zone_notice("Entraste em: dark-forest"));
        assert!(is_zone_notice("De volta ao vale."));
        // Um toast normal continua a ser um toast.
        assert!(!is_zone_notice("PARRY!"));
        assert!(!is_zone_notice("+3 madeira"));
    }

    #[test]
    fn test_next_tab_cycles() {
        assert_eq!(next_tab(0, 1), 1);
        assert_eq!(next_tab(3, 1), 0);
        assert_eq!(next_tab(0, -1), 3);
        assert_eq!(next_tab(1, 4), 1);
    }

    #[test]
    fn test_toast_constants_sane() {
        assert!((1..=6).contains(&TOAST_CAP));
        assert!((2.0..=5.0).contains(&TOAST_LIFETIME));
    }

    /// R2-G2: o painel de viagem rápida [G] conta como menu aberto —
    /// `any()` é a porta de input de movimento/melee/hotbar.
    #[test]
    fn test_menus_open_any_includes_travel_panel() {
        let mut menus = MenusOpen::default();
        assert!(!menus.any(), "tudo fechado = input livre");
        menus.travel = true;
        assert!(menus.any(), "painel de viagem rouba o input");
        menus.travel = false;
        // O mapa regional [M] também rouba input (é um menu): o [L] que o
        // fecha não pode disparar a guarda nem o WASD andar.
        menus.map = true;
        assert!(menus.any(), "mapa regional rouba o input");
        menus.map = false;
        menus.modal = true;
        assert!(menus.any());
        menus.modal = false;
        menus.shop = true;
        assert!(menus.any());
    }
}
