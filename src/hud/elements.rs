//! HUD element builders: everything spawned from world tags
//! (`Minimap`, `Compass`, `WaypointArrow`, `DialogueBalloon`, …). Os
//! builders legacy do simple-rpg (`HealthBar`, `TabbedModal`, `ResourceChip`, …)
//! saíram: o HUD do jogo é declarativo (`<UiRoot>`).

use bevy::prelude::*;
use bevy::ui::BoxShadow;
use bevy::ui::widget::ImageNode;

use super::assets::{HudAssets, centered_at, label, panel_shadow};
use super::compass::{CompassDistance, CompassLetter, CompassTick};
use super::interact::{BALLOON_DURATION, HudBalloon};
use super::minimap::{MinimapAnchor, MinimapArrow, MinimapDot, MinimapRange};

fn attr<'a>(attrs: &'a [(String, String)], name: &str) -> Option<&'a str> {
    attrs
        .iter()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.as_str())
}

/// Sombra quente do tema Tinta Quente (#0c0a0988): cai para baixo e
/// espalha — a mesma assinatura dos cards declarativos.
fn warm_shadow() -> BoxShadow {
    BoxShadow::new(
        Color::srgba(0.047, 0.039, 0.035, 0.53),
        Val::Px(0.0),
        Val::Px(4.0),
        Val::ZERO,
        Val::Px(10.0),
    )
}

/// Build every deferred HUD element. `tag` is the lowercased original tag.
pub fn spawn_hud(world: &mut World, tag: &str, attrs: &[(String, String)]) {
    let hud = HudAssets::get(world);
    match tag {
        "hudscreenlayer" => {
            // Root layer + the two widgets the original renders from the DOM
            // (help pill and action slots) that have no dedicated tag.
            world.spawn((
                Node {
                    position_type: PositionType::Absolute,
                    top: Val::Px(0.0),
                    left: Val::Px(0.0),
                    width: Val::Percent(100.0),
                    height: Val::Percent(100.0),
                    ..Default::default()
                },
                bevy::ui::FocusPolicy::Pass,
                Name::new("hud:layer"),
            ));
            // Os antigos slots de acção saíram daqui: a barra de habilidades
            // é agora `<UiCooldown>` no HUD declarativo (`src/ui`), onde a
            // veladura de recarga e o aro de "pronta" são folha de estilo.
        }
        "minimap" => {
            let range = attr(attrs, "range")
                .and_then(|v| v.parse::<f32>().ok())
                .unwrap_or(60.0);
            use super::minimap::{MINIMAP_H, MINIMAP_W, MinimapThreat};
            // Canto inferior esquerdo, como em qualquer jogo de mundo aberto
            // que se leia: o olhar do jogador vive no centro-baixo e o mapa é
            // a única coisa permanente que ele consulta a meio da corrida.
            //
            // Rectângulo de cantos redondos (não disco): é o que deixa o
            // `Overflow::clip` funcionar — e é o clip que permite o mapa
            // DESLIZAR por baixo da moldura em vez de ser um brasão estático.
            world
                .spawn((
                    Node {
                        position_type: PositionType::Absolute,
                        bottom: Val::Px(18.0),
                        left: Val::Px(18.0),
                        width: Val::Px(MINIMAP_W),
                        height: Val::Px(MINIMAP_H),
                        border: UiRect::all(Val::Px(1.5)),
                        border_radius: BorderRadius::all(Val::Px(7.0)),
                        overflow: Overflow::clip(),
                        ..Default::default()
                    },
                    // Vidro umber: escuro o bastante para o painel se
                    // distinguir do mundo, translúcido o bastante para não
                    // ser um bloco morto no canto.
                    BackgroundColor(Color::srgba(0.11, 0.075, 0.045, 0.78)),
                    BorderColor::all(Color::srgba(0.957, 0.925, 0.847, 0.4)),
                    Name::new("hud:minimap"),
                    MinimapRange(range),
                    super::HideInInterior,
                    panel_shadow(),
                ))
                .with_children(|map| {
                    // Rosa dos ventos: só o N ganha letra (é o Norte que se
                    // procura num relance); os outros pontos são traços.
                    map.spawn((
                        centered_at(Val::Percent(50.0), Val::Px(3.0)),
                        UiTransform::from_translation(Val2::new(Val::Percent(-50.0), Val::ZERO)),
                        label(&hud, "N", 11.0, Color::srgba(0.98, 0.88, 0.62, 0.95)),
                    ));
                    for (left, top, w, h) in [
                        (Val::Percent(97.0), Val::Percent(50.0), 6.0, 2.0),
                        (Val::Percent(50.0), Val::Percent(97.0), 2.0, 6.0),
                        (Val::Percent(3.0), Val::Percent(50.0), 6.0, 2.0),
                    ] {
                        map.spawn((
                            Node {
                                position_type: PositionType::Absolute,
                                left,
                                top,
                                width: Val::Px(w),
                                height: Val::Px(h),
                                border_radius: BorderRadius::all(Val::Px(1.0)),
                                ..Default::default()
                            },
                            UiTransform::from_translation(Val2::new(
                                Val::Percent(-50.0),
                                Val::Percent(-50.0),
                            )),
                            BackgroundColor(Color::srgba(0.957, 0.925, 0.847, 0.5)),
                        ));
                    }
                    // Hostis: aros vermelhos, POR BAIXO dos pontos de quest
                    // na ordem de spawn (desenham primeiro). Doze chegam para
                    // qualquer enquadramento honesto.
                    for _ in 0..12 {
                        map.spawn((
                            Node {
                                position_type: PositionType::Absolute,
                                left: Val::Percent(50.0),
                                top: Val::Percent(50.0),
                                margin: UiRect::px(-3.5, 0.0, -3.5, 0.0),
                                width: Val::Px(7.0),
                                height: Val::Px(7.0),
                                border_radius: BorderRadius::MAX,
                                ..Default::default()
                            },
                            BackgroundColor(Color::srgb(0.788, 0.294, 0.259)),
                            Outline::new(
                                Val::Px(1.0),
                                Val::ZERO,
                                Color::srgba(0.07, 0.05, 0.03, 0.75),
                            ),
                            UiTransform::IDENTITY,
                            Visibility::Hidden,
                            MinimapThreat,
                        ));
                    }
                    // Quest markers: losango dourado com "!" de tinta
                    // (gerado em `assets.rs`) — o vocabulário universal de
                    // "objectivo aqui", legível sobre terreno claro e
                    // escuro; a contagem vive no tracker.
                    for _ in 1..=6 {
                        map.spawn((
                            Node {
                                position_type: PositionType::Absolute,
                                left: Val::Percent(50.0),
                                top: Val::Percent(50.0),
                                margin: UiRect::px(-6.0, 0.0, -6.0, 0.0),
                                width: Val::Px(12.0),
                                height: Val::Px(12.0),
                                ..Default::default()
                            },
                            ImageNode {
                                image: hud.quest_marker.clone(),
                                ..Default::default()
                            },
                            UiTransform::IDENTITY,
                            Visibility::Hidden,
                            MinimapDot,
                        ));
                    }
                    // Âncora do marco assinado (A Nota): pin de mapa dourado
                    // — par de vocabulário com o "!" ("quest = exclamação,
                    // destino = pin"), ambos ícones e não pontos crus.
                    map.spawn((
                        Node {
                            position_type: PositionType::Absolute,
                            left: Val::Percent(50.0),
                            top: Val::Percent(50.0),
                            margin: UiRect::px(-6.0, 0.0, -6.0, 0.0),
                            width: Val::Px(12.0),
                            height: Val::Px(12.0),
                            ..Default::default()
                        },
                        ImageNode {
                            image: hud.map_pin.clone(),
                            ..Default::default()
                        },
                        UiTransform::IDENTITY,
                        Visibility::Hidden,
                        MinimapAnchor,
                    ));
                    // Marcos da Nota disponíveis: losangos de tinta-teal
                    // (rotação fixa 45°) — "cartografia aqui", o vocabulário
                    // que as atalaias REVELOAM. Menores que o pin assinado.
                    for _ in 0..super::minimap::LANDMARK_BLIPS {
                        map.spawn((
                            Node {
                                position_type: PositionType::Absolute,
                                left: Val::Percent(50.0),
                                top: Val::Percent(50.0),
                                margin: UiRect::px(-4.0, 0.0, -4.0, 0.0),
                                width: Val::Px(8.0),
                                height: Val::Px(8.0),
                                border_radius: BorderRadius::all(Val::Px(1.5)),
                                ..Default::default()
                            },
                            BackgroundColor(Color::srgba(0.35, 0.8, 0.78, 0.95)),
                            Outline::new(
                                Val::Px(1.0),
                                Val::ZERO,
                                Color::srgba(0.05, 0.12, 0.12, 0.8),
                            ),
                            UiTransform {
                                rotation: Rot2::radians(std::f32::consts::FRAC_PI_4),
                                ..Default::default()
                            },
                            Visibility::Hidden,
                            super::minimap::MinimapLandmark,
                        ));
                    }
                    // Atalaias: losango maior, com o núcleo claro — é a
                    // estrutura que organiza a exploração; tem de se ler
                    // primeiro.
                    for _ in 0..super::minimap::TOWER_BLIPS {
                        map.spawn((
                            Node {
                                position_type: PositionType::Absolute,
                                left: Val::Percent(50.0),
                                top: Val::Percent(50.0),
                                margin: UiRect::px(-5.5, 0.0, -5.5, 0.0),
                                width: Val::Px(11.0),
                                height: Val::Px(11.0),
                                border_radius: BorderRadius::all(Val::Px(2.0)),
                                ..Default::default()
                            },
                            BackgroundColor(Color::srgb(0.42, 0.92, 0.9)),
                            Outline::new(
                                Val::Px(1.5),
                                Val::ZERO,
                                Color::srgba(0.05, 0.15, 0.15, 0.9),
                            ),
                            UiTransform {
                                rotation: Rot2::radians(std::f32::consts::FRAC_PI_4),
                                ..Default::default()
                            },
                            Visibility::Hidden,
                            super::minimap::MinimapTower,
                        ));
                    }
                    // Player arrow: puck escuro com o triângulo por cima —
                    // desenhado POR ÚLTIMO, portanto sempre por cima dos
                    // blips. Fica sempre no centro; é o mapa que desliza.
                    map.spawn((
                        Node {
                            position_type: PositionType::Absolute,
                            left: Val::Percent(50.0),
                            top: Val::Percent(50.0),
                            margin: UiRect::px(-11.0, 0.0, -11.0, 0.0),
                            width: Val::Px(22.0),
                            height: Val::Px(22.0),
                            justify_content: JustifyContent::Center,
                            align_items: AlignItems::Center,
                            border: UiRect::all(Val::Px(1.5)),
                            border_radius: BorderRadius::MAX,
                            ..Default::default()
                        },
                        BackgroundColor(Color::srgba(0.07, 0.05, 0.03, 0.86)),
                        BorderColor::all(Color::srgba(0.957, 0.925, 0.847, 0.75)),
                        UiTransform::IDENTITY,
                        MinimapArrow,
                    ))
                    .with_children(|puck| {
                        puck.spawn((
                            Node {
                                width: Val::Px(15.0),
                                height: Val::Px(15.0),
                                ..Default::default()
                            },
                            ImageNode {
                                image: hud.arrow.clone(),
                                ..Default::default()
                            },
                        ));
                    });
                });
        }
        "compass" => {
            // Régua de compasso no topo-centro: letras + réguas deslizam com
            // o rumo da câmara (`hud::compass::hud_compass_update`) e cada
            // sector de 45° mostra a distância ao NPC mais próximo. Removida
            // no passe de polimento, RESTAURADA a pedido do autor — a faixa
            // de céu em cima é dela.
            world
                .spawn((
                    Node {
                        position_type: PositionType::Absolute,
                        top: Val::Px(10.0),
                        left: Val::Px(0.0),
                        right: Val::Px(0.0),
                        justify_content: JustifyContent::Center,
                        ..Default::default()
                    },
                    Name::new("hud:compass"),
                    super::HideInInterior,
                ))
                .with_children(|wrap| {
                    const DIRECTIONS: [(&str, f32); 8] = [
                        ("N", 0.0),
                        ("NE", 45.0),
                        ("E", 90.0),
                        ("SE", 135.0),
                        ("S", 180.0),
                        ("SW", 225.0),
                        ("W", 270.0),
                        ("NW", 315.0),
                    ];
                    // Letters live INSIDE the strip so their absolute left is
                    // relative to it (wrap is full-width, the strip is not).
                    wrap.spawn((
                        Node {
                            width: Val::Px(460.0),
                            height: Val::Px(34.0),
                            border_radius: BorderRadius::all(Val::Px(17.0)),
                            ..Default::default()
                        },
                        BackgroundColor(Color::srgba(0.03, 0.03, 0.025, 0.62)),
                        ImageNode {
                            image: hud.panel_gradient.clone(),
                            color: Color::srgba(0.10, 0.095, 0.085, 0.55),
                            ..Default::default()
                        },
                        BorderColor::all(Color::srgba(1.0, 0.96, 0.85, 0.12)),
                        panel_shadow(),
                    ))
                    .with_children(|strip| {
                        // Center caret (static): the heading marker.
                        strip.spawn((
                            Node {
                                position_type: PositionType::Absolute,
                                left: Val::Percent(50.0),
                                top: Val::Px(2.0),
                                width: Val::Px(2.0),
                                height: Val::Px(8.0),
                                border_radius: BorderRadius::all(Val::Px(1.0)),
                                ..Default::default()
                            },
                            BackgroundColor(Color::srgb(0.95, 0.78, 0.25)),
                        ));
                        for (name, bearing) in DIRECTIONS {
                            let color = if name == "N" {
                                Color::srgb(0.95, 0.78, 0.25)
                            } else {
                                Color::srgb(0.92, 0.92, 0.88)
                            };
                            strip.spawn((
                                centered_at(Val::Px(230.0), Val::Px(4.0)),
                                UiTransform::from_translation(Val2::new(
                                    Val::Percent(-50.0),
                                    Val::ZERO,
                                )),
                                label(&hud, name, 13.0, color),
                                CompassLetter {
                                    bearing_deg: bearing,
                                },
                            ));
                            strip.spawn((
                                centered_at(Val::Px(230.0), Val::Px(19.0)),
                                UiTransform::from_translation(Val2::new(
                                    Val::Percent(-50.0),
                                    Val::ZERO,
                                )),
                                label(&hud, "", 9.0, Color::srgba(1.0, 0.92, 0.7, 0.85)),
                                CompassDistance {
                                    bearing_deg: bearing,
                                },
                            ));
                        }
                        // Tick marks every 22.5°.
                        for i in 0..16 {
                            let bearing = i as f32 * 22.5;
                            let tall = i % 2 == 0;
                            strip.spawn((
                                Node {
                                    position_type: PositionType::Absolute,
                                    left: Val::Px(230.0),
                                    top: Val::Px(0.0),
                                    width: Val::Px(1.5),
                                    height: Val::Px(if tall { 5.0 } else { 3.0 }),
                                    ..Default::default()
                                },
                                BackgroundColor(Color::srgba(
                                    1.0,
                                    1.0,
                                    0.95,
                                    if tall { 0.4 } else { 0.22 },
                                )),
                                CompassTick {
                                    bearing_deg: bearing,
                                },
                            ));
                        }
                        // Marcos da Nota disponíveis (atalaias + biomas
                        // revelados): losango teal na faixa — "vale a pena
                        // ver para aí". O slot atribui o rumo por frame
                        // (`hud_compass_landmarks_update`).
                        for slot in 0..super::compass::COMPASS_LANDMARK_SLOTS {
                            strip.spawn((
                                Node {
                                    position_type: PositionType::Absolute,
                                    left: Val::Px(-100.0),
                                    top: Val::Px(25.0),
                                    width: Val::Px(7.0),
                                    height: Val::Px(7.0),
                                    border_radius: BorderRadius::all(Val::Px(1.5)),
                                    ..Default::default()
                                },
                                BackgroundColor(Color::srgba(0.35, 0.8, 0.78, 0.95)),
                                Outline::new(
                                    Val::Px(1.0),
                                    Val::ZERO,
                                    Color::srgba(0.05, 0.12, 0.12, 0.8),
                                ),
                                UiTransform {
                                    rotation: Rot2::radians(std::f32::consts::FRAC_PI_4),
                                    ..Default::default()
                                },
                                Visibility::Hidden,
                                super::compass::CompassLandmark { slot },
                            ));
                        }
                    });
                });
        }
        "dialogueballoon" => {
            // Mesma voz do prompt: tinta escura translúcida + fio dourado +
            // papel. O TEXTO tem de ficar no PRIMEIRO filho (o fluxo de
            // diálogo e o countdown escrevem nele directamente).
            let card = Color::srgba(0.110, 0.098, 0.090, 0.94); // #1c1917f0
            let gold_edge = Color::srgba(0.718, 0.643, 0.467, 0.50);
            let paper = Color::srgb(0.933, 0.914, 0.863); // #eee9dc
            world
                .spawn((
                    Node {
                        position_type: PositionType::Absolute,
                        bottom: Val::Px(180.0),
                        left: Val::Px(0.0),
                        right: Val::Px(0.0),
                        justify_content: JustifyContent::Center,
                        ..Default::default()
                    },
                    Visibility::Hidden,
                    HudBalloon {
                        timer: BALLOON_DURATION,
                    },
                    Name::new("hud:balloon"),
                ))
                .with_children(|wrap| {
                    wrap.spawn((
                        Node {
                            max_width: Val::Px(520.0),
                            padding: UiRect::axes(Val::Px(16.0), Val::Px(10.0)),
                            border: UiRect::all(Val::Px(1.0)),
                            border_radius: BorderRadius::all(Val::Px(9.0)),
                            ..Default::default()
                        },
                        BackgroundColor(card),
                        BorderColor::all(gold_edge),
                        warm_shadow(),
                        Text::new("…"),
                        TextColor(paper),
                        TextFont {
                            font: hud.font.clone().into(),
                            font_size: 15.0.into(),
                            ..Default::default()
                        },
                    ));
                });
        }
        "waypointarrow" => {
            let margin = attr(attrs, "margin")
                .and_then(|v| v.parse::<f32>().ok())
                .unwrap_or(super::waypoint::DEFAULT_MARGIN);
            super::waypoint::spawn_waypoint_arrow(world, &hud, margin);
        }
        other => {
            bevy::log::warn!("hud: unhandled element `{other}` — skipped");
        }
    }
}
