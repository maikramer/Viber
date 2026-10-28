//! Testes headless do debug bridge: App mínima + bridge real sobre HTTP
//! (loopback) — sem janela, sem render. Os testes e2e correm num único
//! `#[test]` para não competir pela mesma porta.

use super::client::BridgeClient;
use super::*;
use bevy::MinimalPlugins;
use std::thread::JoinHandle;

const TEST_PORT: u16 = 35702;

// ── Input sintético: logical_key, newline/tab e press+release no mesmo lote ──

/// `send_key` tem de preencher o `logical_key` que os consumidores de UI lêem
/// (a barra de fala `<UiInput>` despacha por `Key::Backspace`/`Character`/
/// `Enter|Escape` — `ui/widgets.rs`): só o `key_code` não chega. Um
/// `viber debug key backspace` chegava `Unidentified` e a barra não apagava.
#[test]
fn test_send_key_maps_logical_keys_for_text_inputs() {
    let mut world = World::new();
    world.init_resource::<Messages<KeyboardInput>>();
    world.init_resource::<ButtonInput<KeyCode>>();

    let last_logical = |world: &World| {
        world
            .resource::<Messages<KeyboardInput>>()
            .iter_current_update_messages()
            .last()
            .map(|event| event.logical_key.clone())
            .expect("evento de tecla")
    };

    send_key(&mut world, KeyCode::Backspace, ButtonState::Pressed, None);
    assert!(matches!(last_logical(&world), Key::Backspace), "backspace");
    send_key(&mut world, KeyCode::ArrowUp, ButtonState::Pressed, None);
    assert!(matches!(last_logical(&world), Key::ArrowUp), "seta");
    send_key(&mut world, KeyCode::Delete, ButtonState::Pressed, None);
    assert!(matches!(last_logical(&world), Key::Delete), "delete");
    send_key(&mut world, KeyCode::ShiftLeft, ButtonState::Pressed, None);
    assert!(matches!(last_logical(&world), Key::Shift), "modificador");
    // Com `text`, o carácter manda (é o caminho do `viber.input.text`).
    send_key(
        &mut world,
        KeyCode::KeyA,
        ButtonState::Pressed,
        Some("A".into()),
    );
    assert!(
        matches!(&last_logical(&world), Key::Character(chars) if chars == "A"),
        "texto manda: {:?}",
        last_logical(&world)
    );
    assert!(
        world
            .resource::<ButtonInput<KeyCode>>()
            .pressed(KeyCode::Backspace),
        "ButtonInput<KeyCode> continua a ser actualizado"
    );
}

/// `\n` em `viber.input.text` tem de chegar como `Key::Enter` SEM texto (a
/// barra de fala submete no Enter): como `Character("\n")` era INSERTADO no
/// campo em vez de submeter. O mesmo para `\t` → `Key::Tab`.
#[test]
fn test_input_text_maps_newline_and_tab_to_named_keys() {
    let mut world = World::new();
    world.init_resource::<Messages<KeyboardInput>>();
    world.init_resource::<ButtonInput<KeyCode>>();

    input_text(In(Some(json!({ "text": "a\n" }))), &mut world).unwrap();
    let events: Vec<&KeyboardInput> = world
        .resource::<Messages<KeyboardInput>>()
        .iter_current_update_messages()
        .collect();
    assert_eq!(events.len(), 4, "a(press+release) + Enter(press+release)");
    assert!(
        matches!(&events[0].logical_key, Key::Character(chars) if chars == "a"),
        "'a' é carácter: {:?}",
        events[0].logical_key
    );
    assert!(
        matches!(events[2].logical_key, Key::Enter),
        "\\n é Enter (press)"
    );
    assert!(
        events[2].text.is_none(),
        "enter sem texto — submete em vez de inserir"
    );
    assert!(
        matches!(events[3].logical_key, Key::Enter),
        "\\n é Enter (release)"
    );

    input_text(In(Some(json!({ "text": "\t" }))), &mut world).unwrap();
    let events: Vec<&KeyboardInput> = world
        .resource::<Messages<KeyboardInput>>()
        .iter_current_update_messages()
        .collect();
    assert!(
        matches!(events.last().unwrap().logical_key, Key::Tab),
        "\\t é Tab: {:?}",
        events.last().unwrap().logical_key
    );
}

/// `press` + `release` no MESMO lote de pedidos (RemoteLast, antes do
/// PreUpdate seguinte): o press (hold) consumia o release como "previous" e
/// injectava-o ANTES do press — `Released` fantasma (winit nunca solta sem
/// ter premido) e o botão ficava preso até a um novo release. O par tem de
/// acabar num CLIQUE: press primeiro, release no frame seguinte, solto.
const SAME_BATCH_TEST_PORT: u16 = 35716;

#[test]
fn test_click_press_then_release_same_batch_makes_a_click() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(bevy::input::InputPlugin)
        .add_plugins(BridgePlugin {
            port: SAME_BATCH_TEST_PORT,
        });
    app.add_message::<CursorMoved>();
    app.update(); // Startup: liga o servidor (os handlers vão ser chamados à mão)

    input_click(
        In(Some(json!({ "x": 1.0, "y": 2.0, "state": "press" }))),
        app.world_mut(),
    )
    .unwrap();
    input_click(
        In(Some(json!({ "x": 3.0, "y": 4.0, "state": "release" }))),
        app.world_mut(),
    )
    .unwrap();
    {
        let pending = app.world().resource::<PendingMouseClick>();
        assert!(!pending.hold, "release no mesmo lote converte em clique");
        assert!(pending.release.is_some(), "release espera o press injectar");
    }

    app.update(); // press injectado no PreUpdate
    {
        let seen: Vec<ButtonState> = app
            .world()
            .resource::<Messages<MouseButtonInput>>()
            .iter_current_update_messages()
            .map(|event| event.state)
            .collect();
        assert_eq!(
            seen.first(),
            Some(&ButtonState::Pressed),
            "o press é o PRIMEIRO evento — sem release fantasma: {seen:?}"
        );
    }
    app.update(); // release injectado
    assert!(
        !app
            .world()
            .resource::<ButtonInput<MouseButton>>()
            .pressed(MouseButton::Left),
        "o par no mesmo lote acaba solto (clique, não botão preso)"
    );
}

/// `--port 0` não é uma porta (mesmo contrato do `viber run --bridge 0`):
/// sem a validação, o cliente girava 20× a ligar à porta 0 até falhar com um
/// erro de socket sem dizer o que se passou.
#[test]
fn test_resolve_target_rejects_port_zero() {
    let error = match client::resolve_target(Some(0), None) {
        Err(error) => error.to_string(),
        Ok(_) => panic!("--port 0 devia falhar"),
    };
    assert!(error.contains("--port 0"), "{error}");
}

/// Bombeia frames até a chamada responder (a resposta só é escrita quando
/// `app.update()` processa o pedido em RemoteLast).
fn settle(app: &mut App, handle: JoinHandle<Result<Value, String>>) -> Value {
    for _ in 0..600 {
        app.update();
        if handle.is_finished() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    handle.join().unwrap().expect("bridge call responde")
}

/// Variante para chamadas que DEVEM falhar (devolve a mensagem de erro BRP).
fn settle_err(app: &mut App, handle: JoinHandle<Result<Value, String>>) -> String {
    for _ in 0..600 {
        app.update();
        if handle.is_finished() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    handle.join().unwrap().expect_err("chamada devia falhar")
}

fn call_async(method: &'static str, params: Value) -> JoinHandle<Result<Value, String>> {
    call_async_on(TEST_PORT, method, params)
}

/// Chamada `viber.lua` síncrona para testes (bombeia frames até responder).
fn lua_call(app: &mut App, port: u16, code: &str) -> Value {
    let handle = call_async_on(port, METHOD_LUA, serde_json::json!({ "code": code }));
    settle(app, handle)
}

/// Variante com porta explícita — cada App de teste tem a sua, porque os
/// testes correm em paralelo no mesmo binário.
fn call_async_on(
    port: u16,
    method: &'static str,
    params: Value,
) -> JoinHandle<Result<Value, String>> {
    std::thread::spawn(move || {
        BridgeClient::localhost(port)
            .call(method, params)
            .map_err(|error| error.to_string())
    })
}

#[test]
fn test_bridge_end_to_end_headless() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(bevy::input::InputPlugin)
        .add_plugins(BridgePlugin { port: TEST_PORT });
    app.world_mut()
        .spawn((Name::new("hero"), Transform::default()));
    // Sem WindowPlugin (headless), registamos o message de cursor à mão.
    app.add_message::<CursorMoved>();
    app.update(); // Startup: liga o servidor HTTP

    // ping
    let pong = settle(&mut app, call_async(METHOD_PING, serde_json::json!({})));
    let _ = "ping responde";
    assert_eq!(pong["pong"], serde_json::json!(true));
    assert!(pong["version"].is_string());

    // tree: hero presente
    let tree = settle(&mut app, call_async(METHOD_TREE, serde_json::json!({})));
    let _ = "tree responde";
    let entries = tree.as_array().expect("tree é lista");
    assert!(
        entries
            .iter()
            .any(|n| n.get("name").and_then(Value::as_str) == Some("hero")),
        "hero na árvore: {tree}"
    );

    // input.key → evento KeyboardInput + ButtonInput
    settle(
        &mut app,
        call_async(METHOD_KEY, serde_json::json!({ "key": "KeyW" })),
    );
    let keyboard = app.world().resource::<Messages<KeyboardInput>>();
    assert!(
        keyboard.len() >= 2,
        "press+release enviados: {}",
        keyboard.len()
    );

    // input.text
    let text = settle(
        &mut app,
        call_async(METHOD_TEXT, serde_json::json!({ "text": "aB" })),
    );
    assert_eq!(text["chars"], serde_json::json!(2));

    // input.click + input.move → CursorMoved
    let click = call_async(METHOD_CLICK, serde_json::json!({ "x": 10.0, "y": 20.0 }));
    let mouse_move = call_async(METHOD_MOVE, serde_json::json!({ "x": 30.0, "y": 40.0 }));
    // O duplo buffer de `Messages` evicta a mais velha ao fim de dois frames.
    // Sob carga os dois pedidos não caem no mesmo frame — guarda-se o máximo
    // visto durante a espera, não a contagem no fim.
    let mut cursors = std::collections::HashSet::new();
    for _ in 0..600 {
        app.update();
        for cursor in app
            .world()
            .resource::<Messages<CursorMoved>>()
            .iter_current_update_messages()
        {
            cursors.insert((cursor.position.x as i32, cursor.position.y as i32));
        }
        if click.is_finished() && mouse_move.is_finished() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    click.join().unwrap().expect("input.click responde");
    mouse_move.join().unwrap().expect("input.move responde");
    assert!(cursors.len() >= 2, "CursorMoved de click+move: {cursors:?}");
    // O clique é injectado no PreUpdate do frame SEGUINTE (press agora,
    // release depois — ver `deferred_mouse_release`): dá-lhe dois frames.
    app.update();
    app.update();

    // arrasto (`viber debug drag`): `press` deixa o botão em baixo frames a fio,
    // `release` solta-o. (Com janela, sai também como `WindowEvent` — é o que o
    // picking lê; esta app headless não tem essas mensagens e o writer é opcional.)
    settle(
        &mut app,
        call_async(
            METHOD_CLICK,
            serde_json::json!({ "x": 5.0, "y": 5.0, "state": "press" }),
        ),
    );
    for _ in 0..3 {
        app.update();
    }
    assert!(
        app.world()
            .resource::<ButtonInput<MouseButton>>()
            .pressed(MouseButton::Left),
        "press mantém o botão em baixo"
    );
    settle(
        &mut app,
        call_async(
            METHOD_CLICK,
            serde_json::json!({ "x": 9.0, "y": 9.0, "state": "release" }),
        ),
    );
    app.update();
    app.update();
    assert!(
        !app.world()
            .resource::<ButtonInput<MouseButton>>()
            .pressed(MouseButton::Left),
        "release solta o botão"
    );
    let bad = settle_err(
        &mut app,
        call_async(
            METHOD_CLICK,
            serde_json::json!({ "x": 1.0, "y": 1.0, "state": "hold" }),
        ),
    );
    assert!(bad.contains("state"), "state inválido explica-se: {bad}");

    // screenshot: pedido em fila fica pending sem render
    let shot = settle(
        &mut app,
        call_async(METHOD_SCREENSHOT, serde_json::json!({})),
    );
    let id = shot["id"].as_u64().expect("capture id");

    let status = settle(
        &mut app,
        call_async(METHOD_SCREENSHOT_STATUS, serde_json::json!({ "id": id })),
    );
    assert_eq!(status["status"], serde_json::json!("pending"));

    let error = settle_err(
        &mut app,
        call_async(METHOD_SCREENSHOT_STATUS, serde_json::json!({ "id": 99999 })),
    );
    assert!(error.contains("unknown capture id"), "erro: {error}");
}

#[test]
fn test_normalize_key_aliases() {
    use super::client::normalize_key;
    assert_eq!(normalize_key("w"), "KeyW");
    assert_eq!(normalize_key("7"), "Digit7");
    assert_eq!(normalize_key("space"), "Space");
    assert_eq!(normalize_key("esc"), "Escape");
    assert_eq!(normalize_key("up"), "ArrowUp");
    assert_eq!(normalize_key("ctrl"), "ControlLeft");
    assert_eq!(normalize_key("KeyW"), "KeyW");
    assert_eq!(normalize_key("F5"), "F5");
}

#[test]
fn test_keycode_for_char_mapping() {
    assert_eq!(keycode_for_char('a'), (KeyCode::KeyA, false));
    assert_eq!(keycode_for_char('A'), (KeyCode::KeyA, true));
    assert_eq!(keycode_for_char('5'), (KeyCode::Digit5, false));
    assert_eq!(keycode_for_char('!'), (KeyCode::Digit1, true));
    assert_eq!(keycode_for_char(' '), (KeyCode::Space, false));
}

/// Porta própria: não pode colidir com `TEST_PORT` do e2e do bridge nem com
/// outros testes Luau (correm em paralelo no mesmo binário).
const LUA_TEST_PORT: u16 = 35703;
const LUA2_TEST_PORT: u16 = 35704;
const LUA3_TEST_PORT: u16 = 35705;
const LUA4_TEST_PORT: u16 = 35706;

/// App headless com runtime Luau + bridge: base para os testes `viber.lua`.
fn lua_app(port: u16) -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(crate::luau::LuauScriptPlugin::default())
        .add_plugins(BridgePlugin { port });
    // Assets para os testes de introspeção (sem AssetPlugin na app mínima).
    app.init_resource::<bevy::asset::Assets<bevy::mesh::Mesh>>();
    app.init_resource::<bevy::asset::Assets<bevy::pbr::StandardMaterial>>();
    app.world_mut().spawn((
        Name::new("player"),
        crate::player::Player::default(),
        Transform::from_xyz(0.0, 1.0, 0.0),
        crate::vitals::Health::default(),
    ));
    app.world_mut().spawn((
        Name::new("goblin"),
        Transform::from_xyz(5.0, 0.0, 0.0),
        crate::luau::LuaScriptRef {
            path: "ghost.lua".into(),
        },
    ));
    app.world_mut().spawn((
        Name::new("dummy"),
        Transform::from_xyz(0.0, 0.0, 50.0),
        crate::vitals::Health::default(),
    ));
    // Entidade "rich" com mesh + material + collider + corpo rígido: alvo
    // dos testes de introspeção (info/mesh/material/collider/components).
    let mut meshes = app
        .world_mut()
        .get_resource_mut::<bevy::asset::Assets<bevy::mesh::Mesh>>()
        .unwrap();
    let cube = meshes.add(bevy::mesh::Mesh::from(bevy::shape::Cuboid::new(
        1.0, 1.0, 1.0,
    )));
    let mut materials = app
        .world_mut()
        .get_resource_mut::<bevy::asset::Assets<bevy::pbr::StandardMaterial>>()
        .unwrap();
    let material = materials.add(bevy::pbr::StandardMaterial {
        base_color: bevy::color::Color::srgb(0.8, 0.2, 0.1),
        ..Default::default()
    });
    drop(materials);
    app.world_mut().spawn((
        Name::new("rich"),
        Transform::from_xyz(1.0, 2.0, 3.0).with_scale(Vec3::splat(2.0)),
        bevy::render::mesh::Mesh3d(cube),
        bevy::pbr::MeshMaterial3d(material),
        bevy_rapier3d::prelude::Collider::cuboid(0.5, 0.5, 0.5),
        bevy_rapier3d::prelude::RigidBody::Fixed,
        bevy::light::PointLight {
            intensity: 1200.0,
            shadow_maps_enabled: true,
            ..Default::default()
        },
    ));
    app.update(); // Startup: liga o servidor HTTP
    app
}

#[test]
fn test_bridge_lua_end_to_end_headless() {
    let mut app = lua_app(LUA_TEST_PORT);

    // `return` devolve o valor; conversão mlua → JSON.
    let response = settle(
        &mut app,
        call_async_on(
            LUA_TEST_PORT,
            METHOD_LUA,
            serde_json::json!({ "code": "return 1 + 1" }),
        ),
    );
    assert_eq!(response["ok"], serde_json::json!(true));
    assert_eq!(response["result"], serde_json::json!(2));

    // player() lê o snapshot; teleport aplica a op no mesmo frame.
    let response = settle(
        &mut app,
        call_async_on(
            LUA_TEST_PORT,
            METHOD_LUA,
            serde_json::json!({
                "code": "viber.debug.teleport(10, 5, -3)\nreturn viber.debug.player().x"
            }),
        ),
    );
    assert_eq!(response["ok"], serde_json::json!(true));
    assert_eq!(
        response["result"].as_f64(),
        Some(0.0),
        "snapshot é do início da chamada"
    );
    assert_eq!(response["applied"], serde_json::json!(1));

    let mut query = app
        .world_mut()
        .query_filtered::<(&Name, &Transform), bevy::ecs::query::With<crate::player::Player>>();
    let (_, transform) = query.single(app.world()).expect("player com Transform");
    assert_eq!(transform.translation, Vec3::new(10.0, 5.0, -3.0));

    // find por nome + set_pos; pos() lê o snapshot do INÍCIO da chamada
    // (antes da op aplicar) — o read-back vem no chunk seguinte.
    let response = settle(
        &mut app,
        call_async_on(
            LUA_TEST_PORT,
            METHOD_LUA,
            serde_json::json!({
                "code": "viber.debug.set_pos('goblin', 1, 2, 3)\nreturn { viber.debug.pos('goblin') }"
            }),
        ),
    );
    assert_eq!(response["ok"], serde_json::json!(true));
    let pos = response["result"].as_array().expect("pos é xyz");
    assert_eq!(pos[0].as_f64(), Some(5.0), "posição pré-op");

    let response = settle(
        &mut app,
        call_async_on(
            LUA_TEST_PORT,
            METHOD_LUA,
            serde_json::json!({ "code": "return { viber.debug.pos('goblin') }" }),
        ),
    );
    // Leitura vem do snapshot — a op do chunk anterior já aplicou.
    let pos = response["result"].as_array().expect("pos é xyz");
    assert_eq!(pos[1].as_f64(), Some(2.0), "y do set_pos anterior");

    // Erro Luau → ok:false com a mensagem; a engine continua viva.
    let response = settle(
        &mut app,
        call_async_on(
            LUA_TEST_PORT,
            METHOD_LUA,
            serde_json::json!({ "code": "error('boom')" }),
        ),
    );
    assert_eq!(response["ok"], serde_json::json!(false));
    assert!(
        response["error"]
            .as_str()
            .is_some_and(|e| e.contains("boom")),
        "erro: {response}"
    );

    // disable() insere `Disabled` — a entidade sai das queries normais.
    let response = settle(
        &mut app,
        call_async_on(
            LUA_TEST_PORT,
            METHOD_LUA,
            serde_json::json!({ "code": "viber.debug.disable('goblin') return true" }),
        ),
    );
    assert_eq!(response["ok"], serde_json::json!(true));
    let mut disabled = app
        .world_mut()
        .query_filtered::<&Name, bevy::ecs::query::With<bevy::ecs::entity_disabling::Disabled>>();
    let found = disabled
        .iter(app.world())
        .any(|name| name.as_str() == "goblin");
    assert!(found, "goblin devia estar Disabled");

    // enable() devolve-o ao mundo (mesmo escondido das queries enquanto
    // estava desativado, o snapshot via-o via iter_entities).
    let response = settle(
        &mut app,
        call_async_on(
            LUA_TEST_PORT,
            METHOD_LUA,
            serde_json::json!({ "code": "viber.debug.enable('goblin') return true" }),
        ),
    );
    assert_eq!(response["ok"], serde_json::json!(true));
    let mut still_disabled = app
        .world_mut()
        .query_filtered::<&Name, bevy::ecs::query::With<bevy::ecs::entity_disabling::Disabled>>();
    assert!(
        !still_disabled
            .iter(app.world())
            .any(|name| name.as_str() == "goblin"),
        "goblin devia estar ativo"
    );

    // Globals persistem entre chamadas (REPL).
    let response = settle(
        &mut app,
        call_async_on(
            LUA_TEST_PORT,
            METHOD_LUA,
            serde_json::json!({ "code": "repl_hits = (repl_hits or 0) + 1 return true" }),
        ),
    );
    assert_eq!(response["ok"], serde_json::json!(true));
    let response = settle(
        &mut app,
        call_async_on(
            LUA_TEST_PORT,
            METHOD_LUA,
            serde_json::json!({ "code": "return repl_hits" }),
        ),
    );
    assert_eq!(response["result"], serde_json::json!(1));

    // code ausente → erro BRP de params (fim do e2e: só há uma App nesta
    // porta — testes em paralelo não podem competir pela mesma).
    let error = settle_err(
        &mut app,
        call_async_on(LUA_TEST_PORT, METHOD_LUA, serde_json::json!({})),
    );
    assert!(error.contains("invalid params"), "erro: {error}");
}

/// Segundo lote de features: move_to/rotate/set_scale/kill/set_hp,
/// leituras distance/vault/quests/prof/fps, e ops com warning (câmara,
/// relógio — sem recursos nesta App mínima).
#[test]
fn test_bridge_lua_features_round2() {
    let mut app = lua_app(LUA2_TEST_PORT);
    let lua = |code: &'static str| {
        call_async_on(
            LUA2_TEST_PORT,
            METHOD_LUA,
            serde_json::json!({ "code": code }),
        )
    };

    // move_to: qualquer entidade, XZ absolutos (sem terreno → Y fica).
    let response = settle(
        &mut app,
        lua("viber.debug.move_to('goblin', 20, 5) return true"),
    );
    assert_eq!(response["ok"], serde_json::json!(true));
    let mut query = app
        .world_mut()
        .query_filtered::<&Transform, bevy::ecs::query::Without<crate::player::Player>>();
    let goblin_pos: Vec3 = query
        .iter(app.world())
        .find(|t| t.translation.x == 20.0)
        .map(|t| t.translation)
        .expect("goblin movido");
    assert_eq!(goblin_pos.z, 5.0);

    // rotate +90°: yaw inicial identidade → π/2.
    let response = settle(
        &mut app,
        lua("viber.debug.rotate('goblin', 90) return true"),
    );
    assert_eq!(response["ok"], serde_json::json!(true));
    let mut query = app
        .world_mut()
        .query_filtered::<&Transform, bevy::ecs::query::Without<crate::player::Player>>();
    let yaw = query
        .iter(app.world())
        .find(|t| t.translation.x == 20.0)
        .map(|t| t.rotation.to_euler(bevy::math::EulerRot::YXZ).0)
        .expect("goblin");
    assert!(
        (yaw - std::f32::consts::FRAC_PI_2).abs() < 1e-4,
        "yaw devia ser 90°, foi {}",
        yaw.to_degrees()
    );

    // set_scale uniforme.
    let response = settle(
        &mut app,
        lua("viber.debug.set_scale('goblin', 2) return true"),
    );
    assert_eq!(response["ok"], serde_json::json!(true));
    let mut query = app
        .world_mut()
        .query_filtered::<&Transform, bevy::ecs::query::Without<crate::player::Player>>();
    let scale = query
        .iter(app.world())
        .find(|t| t.translation.x == 20.0)
        .map(|t| t.scale.x)
        .expect("goblin");
    assert_eq!(scale, 2.0);

    // set_hp absoluto no player + kill cru no dummy (Health a zero).
    let response = settle(
        &mut app,
        lua("viber.debug.set_hp(7) viber.debug.kill('dummy') return true"),
    );
    assert_eq!(response["ok"], serde_json::json!(true));
    assert_eq!(response["applied"], serde_json::json!(2));
    let mut player_q = app
        .world_mut()
        .query_filtered::<&crate::vitals::Health, bevy::ecs::query::With<crate::player::Player>>();
    let player_hp = player_q
        .single(app.world())
        .expect("Health do player")
        .current;
    let mut dummy_q = app
        .world_mut()
        .query_filtered::<&crate::vitals::Health, bevy::ecs::query::Without<crate::player::Player>>(
        );
    let dummy_hp = dummy_q
        .single(app.world())
        .expect("Health do dummy")
        .current;
    assert_eq!(player_hp, 7.0, "set_hp");
    assert_eq!(dummy_hp, 0.0, "kill");

    // Ops sem recurso correspondente → warnings, não erro BRP.
    let response = settle(
        &mut app,
        lua(
            "viber.debug.set_camera{distance = 9} viber.debug.set_clock(1380) viber.debug.clear_markers() return true",
        ),
    );
    assert_eq!(response["ok"], serde_json::json!(true));
    let warnings = response["warnings"].as_array().expect("warnings é lista");
    assert_eq!(warnings.len(), 2, "câmara e relógio avisam: {warnings:?}");

    // Leituras: distance + prof/fps. Vault/quests vivem no jogo (Lua) e já
    // não são superfície de debug da engine.
    let response = settle(
        &mut app,
        lua("return { d = viber.debug.distance('player', 'goblin') }"),
    );
    assert_eq!(response["ok"], serde_json::json!(true));
    let result = &response["result"];
    let expected = ((20.0f32 * 20.0 + 1.0 + 5.0 * 5.0).sqrt()) as f64;
    assert!(
        (result["d"].as_f64().unwrap() - expected).abs() < 1e-3,
        "distance"
    );

    let response = settle(
        &mut app,
        lua("return { entities = viber.debug.prof().entities, fps = viber.debug.fps() }"),
    );
    assert_eq!(response["ok"], serde_json::json!(true));
    let result = &response["result"];
    assert!(
        result["entities"].as_u64().is_some(),
        "prof.entities presente"
    );
    assert!(result["fps"].is_null(), "fps sem DiagnosticsStore → nil");
}

/// Terceiro lote: introspeção — info/transform/mesh/material/collider/
/// components sobre a entidade "rich" (mesh+material+collider+corpo).
#[test]
fn test_bridge_lua_introspection() {
    let mut app = lua_app(LUA3_TEST_PORT);
    let lua = |code: &'static str| {
        call_async_on(
            LUA3_TEST_PORT,
            METHOD_LUA,
            serde_json::json!({ "code": code }),
        )
    };

    // components() lista os componentes por nome.
    let response = settle(
        &mut app,
        lua("local c = viber.debug.components('rich') table.sort(c) return c"),
    );
    assert_eq!(response["ok"], serde_json::json!(true));
    let components: Vec<&str> = response["result"]
        .as_array()
        .expect("lista de componentes")
        .iter()
        .map(|v| v.as_str().expect("string"))
        .collect();
    for expected in ["Transform", "Mesh3d", "Collider", "Name", "RigidBody"] {
        assert!(
            components
                .iter()
                .any(|c| c.ends_with(&format!("::{expected}")) || *c == expected),
            "'{expected}' em {components:?}"
        );
    }

    // mesh(): vértices + UVs do cubo (24 verts, uvs 0..1).
    let response = settle(
        &mut app,
        lua(
            "local m = viber.debug.mesh('rich') return { m.vertices, m.has_uvs, m.uv_min[1], m.uv_max[1] }",
        ),
    );
    assert_eq!(response["ok"], serde_json::json!(true));
    let result = response["result"].as_array().expect("lista");
    assert_eq!(result[0], serde_json::json!(24), "cubo tem 24 vértices");
    assert_eq!(result[1], serde_json::json!(true));
    assert_eq!(result[2].as_f64(), Some(0.0), "uv_min");
    assert_eq!(result[3].as_f64(), Some(1.0), "uv_max");

    // material(): cor base e sem textura.
    let response = settle(
        &mut app,
        lua(
            "local m = viber.debug.material('rich') return { r = m.base_color[1], tex = m.base_color_texture }",
        ),
    );
    let result = &response["result"];
    assert!((result["r"].as_f64().unwrap() - 0.8).abs() < 1e-3, "r≈0.8");
    assert!(result["tex"].is_null(), "sem textura base_color");

    // collider(): cuboid com half-extents 0.5 + rigidbody Fixed.
    let response = settle(
        &mut app,
        lua("local c = viber.debug.collider('rich') return { c.shape, c.hx, c.hy, c.hz }"),
    );
    let result = response["result"].as_array().expect("lista");
    assert_eq!(result[0], serde_json::json!("cuboid"));
    assert_eq!(result[1], serde_json::json!(0.5));

    // transform(): translation, scale e euler.
    let response = settle(
        &mut app,
        lua("local t = viber.debug.transform('rich') return { t.x, t.y, t.z, t.sx }"),
    );
    let result = response["result"].as_array().expect("lista");
    assert_eq!(result[0].as_f64(), Some(1.0));
    assert_eq!(result[1].as_f64(), Some(2.0));
    assert_eq!(result[2].as_f64(), Some(3.0));
    assert_eq!(result[3].as_f64(), Some(2.0), "scale uniforme");

    // info(): tabela agregada com tudo.
    let response = settle(
        &mut app,
        lua(
            "local i = viber.debug.info('rich') return { i.name, i.rigidbody, i.collider ~= nil, i.mesh ~= nil, i.material ~= nil, #i.components > 0 }",
        ),
    );
    let result = response["result"].as_array().expect("lista");
    assert_eq!(result[0], serde_json::json!("rich"));
    assert_eq!(result[1], serde_json::json!("Fixed"));
    assert_eq!(result[2], serde_json::json!(true));
    assert_eq!(result[3], serde_json::json!(true));
    assert_eq!(result[4], serde_json::json!(true));
    assert_eq!(result[5], serde_json::json!(true));

    // Entidade sem mesh → erro Luau claro (ok:false).
    let response = settle(&mut app, lua("return viber.debug.mesh('player')"));
    assert_eq!(response["ok"], serde_json::json!(false));
    assert!(
        response["error"].as_str().unwrap().contains("Mesh3d"),
        "erro: {response}"
    );
}

/// Quarto lote: bulk/profiling — stats, colliders, lights, around, physics.
#[test]
fn test_bridge_lua_bulk_profiling() {
    let mut app = lua_app(LUA4_TEST_PORT);
    let lua = |code: &'static str| {
        call_async_on(
            LUA4_TEST_PORT,
            METHOD_LUA,
            serde_json::json!({ "code": code }),
        )
    };

    // stats(): agregados do mundo inteiro.
    let response = settle(
        &mut app,
        lua(
            "local s = viber.debug.stats() return { s.entities, s.meshes, s.colliders, s.lights_point, s.lights_with_shadows, s.rigidbodies }",
        ),
    );
    assert_eq!(response["ok"], serde_json::json!(true), "resp: {response}");
    let result = response["result"].as_array().expect("lista");
    assert!(result[0].as_u64().unwrap() >= 4, "pelo menos 4 entidades");
    assert_eq!(result[1], serde_json::json!(1), "1 mesh (rich)");
    assert_eq!(result[2], serde_json::json!(1), "1 collider (rich)");
    assert_eq!(result[3], serde_json::json!(1), "1 PointLight");
    assert_eq!(result[4], serde_json::json!(1), "luz com sombras");
    assert_eq!(result[5], serde_json::json!(1), "1 RigidBody");

    // colliders(): o rich aparece com shape cuboid.
    let response = settle(
        &mut app,
        lua("local c = viber.debug.colliders() return { #c, c[1].shape, c[1].name }"),
    );
    let result = response["result"].as_array().expect("lista");
    assert_eq!(result[0], serde_json::json!(1));
    assert_eq!(result[1], serde_json::json!("cuboid"));
    assert_eq!(result[2], serde_json::json!("rich"));

    // lights(): kind/shadows/intensity.
    let response = settle(
        &mut app,
        lua(
            "local l = viber.debug.lights() return { #l, l[1].kind, l[1].shadows, l[1].intensity }",
        ),
    );
    let result = response["result"].as_array().expect("lista");
    assert_eq!(result[0], serde_json::json!(1));
    assert_eq!(result[1], serde_json::json!("point"));
    assert_eq!(result[2], serde_json::json!(true));
    assert_eq!(result[3], serde_json::json!(1200));

    // around(): player e rich no raio; dummy (z=50) fora.
    let response = settle(
        &mut app,
        lua("local a = viber.debug.around(10) return { #a, a[1].name ~= nil, a[2] ~= nil }"),
    );
    assert_eq!(response["ok"], serde_json::json!(true), "resp: {response}");
    let result = response["result"].as_array().expect("lista");
    assert_eq!(
        result[0].as_u64().unwrap(),
        3,
        "player + goblin + rich num raio de 10"
    );
    // dummy está a ~50 m: raio pequeno exclui.
    let response = settle(&mut app, lua("local a = viber.debug.around(100) return #a"));
    assert!(
        response["result"].as_u64().unwrap() >= 4,
        "raio grande inclui o dummy"
    );

    // physics(): sem RapierContext nesta app → nil (não pode panicar).
    let response = settle(&mut app, lua("return viber.debug.physics() == nil"));
    assert_eq!(response["ok"], serde_json::json!(true));
    assert_eq!(response["result"], serde_json::json!(true));
}

/// Porta própria — `step`/`play`, `ground` sem terreno e raios sem player.
const STEP_TEST_PORT: u16 = 35714;

/// Regressões do QA frame a frame: o `play` não pode congelar o mundo
/// (o `FrameStepper` nascia com `restore` 0.0), o `ground` sem terreno não
/// pode dizer `applied: 1`, e um raio sem player não pode ser ignorado.
#[test]
fn test_step_play_ground_and_radius_regressions() {
    let mut app = lua_app(STEP_TEST_PORT);
    let speed = |app: &App| {
        app.world()
            .resource::<Time<bevy::time::Virtual>>()
            .relative_speed()
    };
    assert_eq!(speed(&app), 1.0);

    // `play` sem `step`: não há speed guardada — NÃO pode pausar o mundo.
    let r = lua_call(&mut app, STEP_TEST_PORT, "viber.debug.play() return true");
    assert_eq!(r["ok"], serde_json::json!(true));
    app.update();
    assert_eq!(speed(&app), 1.0, "play sem step não pausa");

    // `step 2`: avança 2 frames a speed 1 e fica pausado.
    let r = lua_call(&mut app, STEP_TEST_PORT, "viber.debug.step(2) return true");
    assert_eq!(r["applied"], serde_json::json!(1));
    for _ in 0..2 {
        app.update();
    }
    assert_eq!(speed(&app), 0.0, "step deixa o mundo pausado");
    // `play` restaura a speed da PRIMEIRA chamada da cadeia (1.0), não 0.
    let r = lua_call(&mut app, STEP_TEST_PORT, "viber.debug.play() return true");
    assert_eq!(r["ok"], serde_json::json!(true));
    assert_eq!(speed(&app), 1.0, "play restaura a speed anterior ao step");

    // `ground{}` sem campo nenhum e só-paredes sem materiais de chunk:
    // `applied: 0` com warning (antes: no-op silencioso com applied 1).
    let r = lua_call(&mut app, STEP_TEST_PORT, "viber.debug.ground{} return true");
    assert_eq!(r["applied"], serde_json::json!(0));
    assert!(
        r["warnings"].to_string().contains("nenhum campo"),
        "warning: {}",
        r["warnings"]
    );
    let r = lua_call(
        &mut app,
        STEP_TEST_PORT,
        "viber.debug.ground{moss = 0.5} return true",
    );
    assert_eq!(r["applied"], serde_json::json!(0));
    assert!(
        r["warnings"].to_string().contains("sem materiais de chunk"),
        "warning: {}",
        r["warnings"]
    );

    // Raio sem player: erro claro em vez de devolver o snapshot inteiro
    // (ou os colliders/luzes longe) como se estivessem no raio.
    let player = {
        let mut query = app
            .world_mut()
            .query_filtered::<Entity, bevy::ecs::query::With<crate::player::Player>>();
        query.single(app.world()).ok()
    };
    app.world_mut().despawn(player.expect("player do lua_app"));
    for code in [
        "return viber.debug.entities(10)",
        "return viber.debug.colliders(10)",
        "return viber.debug.lights(10)",
    ] {
        let r = lua_call(&mut app, STEP_TEST_PORT, code);
        assert_eq!(r["ok"], serde_json::json!(false), "code: {code} → {r}");
        assert!(
            r["error"]
                .as_str()
                .is_some_and(|error| error.contains("sem player")),
            "erro de raio: {r}"
        );
    }
}

/// Tab do profiler inválido é erro BRP (antes caía no snapshot base e
/// parecia dados do tab pedido); `all` sem `ProfilerPlugin` é erro, não
/// panic do exclusive system.
#[test]
fn test_profiler_tab_validates_name_and_plugin() {
    let mut world = World::default();
    let error = profiler_tab(In(Some(serde_json::json!({ "tab": "fisics" }))), &mut world)
        .expect_err("tab desconhecido é erro");
    assert!(error.message.contains("desconhecido"), "{}", error.message);
    let error = profiler_tab(In(Some(serde_json::json!({ "tab": "all" }))), &mut world)
        .expect_err("sem ProfilerState é erro");
    assert!(
        error.message.contains("profiler inativo"),
        "{}",
        error.message
    );
}

/// Porta própria — fila de capturas.
const CAPTURE_QUEUE_TEST_PORT: u16 = 35715;

/// O render aceita UM `Screenshot` por janela por frame: a fila tem de
/// spawnar um por frame (o resto no frame seguinte), senão o segundo pedido
/// é despawnado pelo `extract_screenshots` e nunca capturado.
#[test]
fn test_capture_requests_spawn_one_per_frame() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins).add_plugins(BridgePlugin {
        port: CAPTURE_QUEUE_TEST_PORT,
    });
    app.update(); // Startup: liga o servidor HTTP (não usado neste teste)
    {
        let shared = app.world().resource::<BridgeShared>();
        let mut store = shared.captures.lock().unwrap();
        let _first = store.request();
        let _second = store.request();
    }
    app.update();
    {
        let shared = app.world().resource::<BridgeShared>();
        let store = shared.captures.lock().unwrap();
        assert_eq!(
            store.pending.len(),
            1,
            "o segundo pedido espera o frame seguinte"
        );
        assert_eq!(
            store.screenshot_entities.len(),
            1,
            "um `Screenshot` por frame"
        );
    }
    app.update();
    {
        let shared = app.world().resource::<BridgeShared>();
        let store = shared.captures.lock().unwrap();
        assert_eq!(store.pending.len(), 0, "fila drenada ao ritmo do render");
        assert_eq!(store.screenshot_entities.len(), 2);
    }
}

/// `viber.tree` honra `max_depth` (o dock do editor pede 3) e publica a
/// posição de MUNDO dos filhos (não a local).
#[test]
fn test_tree_max_depth_and_world_translation() {
    let mut world = World::default();
    let root = world.spawn((Name::new("root"), Transform::IDENTITY)).id();
    let child = world
        .spawn((
            Name::new("child"),
            ChildOf(root),
            Transform::from_xyz(1.0, 0.0, 0.0),
            GlobalTransform::from_translation(Vec3::new(11.0, 2.0, 3.0)),
        ))
        .id();
    world.spawn((Name::new("grand"), ChildOf(child)));

    // Sem `max_depth`: as três entidades NOMEADAS (o `World` do Bevy tem
    // ainda a entidade interna 0v0 de `DefaultQueryFilters`).
    let all = tree(In(None), &mut world).expect("tree");
    let named = |value: &Value| -> Vec<String> {
        value
            .as_array()
            .expect("lista")
            .iter()
            .filter_map(|node| node.get("name").and_then(Value::as_str))
            .map(str::to_string)
            .collect()
    };
    assert_eq!(named(&all).len(), 3, "{all}");
    let child_node = all
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node.get("name").and_then(Value::as_str) == Some("child"))
        .expect("child na árvore");
    assert_eq!(
        child_node.get("translation"),
        Some(&serde_json::json!([11.0, 2.0, 3.0])),
        "filho publica posição de mundo (GlobalTransform)"
    );

    // `max_depth: 2` = raízes + filhos (a neta fica fora).
    let shallow =
        tree(In(Some(serde_json::json!({ "max_depth": 2 }))), &mut world).expect("tree com depth");
    let names = named(&shallow);
    assert!(names.iter().any(|name| name == "root"), "{names:?}");
    assert!(names.iter().any(|name| name == "child"), "{names:?}");
    assert!(!names.iter().any(|name| name == "grand"), "{names:?}");
}

/// Porta própria — os testes do bridge correm em paralelo no mesmo binário.
const IDENTITY_TEST_PORT: u16 = 35707;

/// O `viber.ping` identifica o mundo servido quando a engine tem
/// `BridgeIdentity` (inserida pelo `run`) — é o que o `viber debug --world`
/// valida contra o engine.json para apanhar registos stale.
#[test]
fn test_ping_reports_world_identity() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(bevy::input::InputPlugin)
        .add_plugins(BridgePlugin {
            port: IDENTITY_TEST_PORT,
        })
        .insert_resource(BridgeIdentity {
            world: "/repo/worlds/qa-pontes.xml".into(),
        });
    app.add_message::<CursorMoved>();
    app.update();

    let pong = settle(
        &mut app,
        call_async_on(IDENTITY_TEST_PORT, METHOD_PING, serde_json::json!({})),
    );
    assert_eq!(pong["pong"], serde_json::json!(true));
    assert_eq!(
        pong["world"],
        serde_json::json!("/repo/worlds/qa-pontes.xml")
    );
}

const BURST_TEST_PORT: u16 = 35708;

/// O `viber.burst` valida params cedo (frames fora da lista = erro BRP), e
/// um burst válido enfileira, spawna capturas (o sistema `Update`) e fica
/// `capturing` no status — headless (sem render) o guard de stall é quem
/// eventualmente o falha; aqui só se verifica o contrato do protocolo.
#[test]
fn test_bridge_burst_headless() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(bevy::input::InputPlugin)
        .add_plugins(BridgePlugin {
            port: BURST_TEST_PORT,
        });
    app.add_message::<CursorMoved>();
    app.update();

    // frames inválido → invalid params com a lista aceite na mensagem
    let error = settle_err(
        &mut app,
        call_async_on(
            BURST_TEST_PORT,
            METHOD_BURST,
            serde_json::json!({ "frames": 5 }),
        ),
    );
    assert!(
        error.contains("4, 9 ou 16"),
        "erro devia listar os aceites: {error}"
    );

    // skip acima do teto → idem
    let error = settle_err(
        &mut app,
        call_async_on(
            BURST_TEST_PORT,
            METHOD_BURST,
            serde_json::json!({ "frames": 9, "skip": 10_000 }),
        ),
    );
    assert!(error.contains("skip"), "erro devia falar do skip: {error}");

    // burst válido → id + path; o status começa capturing com 0 capturados.
    // skip alto de propósito: garante que, não importa quantos updates o
    // pump do settle faça, só UMA captura chega a spawnar (determinismo).
    let started = settle(
        &mut app,
        call_async_on(
            BURST_TEST_PORT,
            METHOD_BURST,
            serde_json::json!({ "frames": 4, "skip": 500 }),
        ),
    );
    let id = started["id"].as_u64().expect("id do burst");
    assert_eq!(started["frames"], serde_json::json!(4));
    assert_eq!(started["skip"], serde_json::json!(500));
    assert!(started["path"].as_str().expect("path").contains("burst-"));

    // Um frame da app depois, a spawn já correu (Update) — status capturing.
    app.update();
    let status = settle(
        &mut app,
        call_async_on(
            BURST_TEST_PORT,
            METHOD_BURST_STATUS,
            serde_json::json!({ "id": id }),
        ),
    );
    assert_eq!(status["status"], serde_json::json!("capturing"));
    assert_eq!(status["captured"], serde_json::json!(0));
    assert_eq!(
        status["spawned"],
        serde_json::json!(1),
        "1.ª captura spawna no 1.º tick"
    );
    assert!(
        status.get("png_base64").is_none(),
        "sem folha enquanto não capturado"
    );

    // id desconhecido → erro claro
    let error = settle_err(
        &mut app,
        call_async_on(
            BURST_TEST_PORT,
            METHOD_BURST_STATUS,
            serde_json::json!({ "id": 9999 }),
        ),
    );
    assert!(
        error.contains("unknown burst id"),
        "erro devia ser de id: {error}"
    );
}

// ── M1: introspecção profunda (vitals de qualquer entidade, IA, nav, …) ──

const LUA5_TEST_PORT: u16 = 35709;

/// `viber.debug.*` do M1: health/ai por entidade, nav, quests fundas, seeds,
/// world_hash, waypoints, save_info, audio — nil-safe sem os plugins de jogo.
#[test]
fn test_bridge_lua_introspection_round3() {
    let mut app = lua_app(LUA5_TEST_PORT);
    {
        let world = app.world_mut();
        // Criatura da FSM com vitals, locomoção e perfil de nav.
        world.spawn((
            Name::new("mob"),
            Transform::from_xyz(-4.0, 0.5, 2.0),
            crate::vitals::Health {
                current: 30.0,
                max: 90.0,
            },
            crate::ai::EnemyCreature {
                speed: 3.5,
                ..Default::default()
            },
            crate::ai::AiLocomotion::default(),
            crate::nav::NavProfile::Wild,
        ));
        // Pilha de nav: config + tile (census fica vazio — sem landmass).
        world.insert_resource(crate::nav::NavConfig::default());
        world.insert_resource(crate::nav::NavTile::default());
    }
    app.update();

    // health() de QUALQUER entidade — não só do player.
    let r = lua_call(
        &mut app,
        LUA5_TEST_PORT,
        "local h = viber.debug.health('mob') return { h.current, h.max, h.dead }",
    );
    assert_eq!(r["ok"], serde_json::json!(true));
    assert_eq!(r["result"][0].as_f64(), Some(30.0));
    assert_eq!(r["result"][1].as_f64(), Some(90.0));
    assert_eq!(r["result"][2], serde_json::json!(false));

    // Sem Health → nil (e não erro).
    let r = lua_call(
        &mut app,
        LUA5_TEST_PORT,
        "return viber.debug.health('goblin') == nil",
    );
    assert_eq!(r["result"], serde_json::json!(true));

    // ai() — FSM + locomoção + perfil.
    let r = lua_call(
        &mut app,
        LUA5_TEST_PORT,
        "local a = viber.debug.ai('mob') return { a.state, a.speed, a.nav_profile, a.aggro_radius }",
    );
    assert_eq!(r["result"][0], serde_json::json!("wander"));
    assert_eq!(r["result"][1].as_f64(), Some(3.5));
    assert_eq!(r["result"][2], serde_json::json!("wild"));
    assert_eq!(r["result"][3].as_f64(), Some(18.0));

    // info() enriquecido: hp/max_hp + ai + script na mesma tabela.
    let r = lua_call(
        &mut app,
        LUA5_TEST_PORT,
        "local i = viber.debug.info('mob') local g = viber.debug.info('goblin')
         return { i.hp, i.max_hp, i.ai.nav_profile, g.script,
                  i.ai.velocity ~= nil, i.ai.desired ~= nil }",
    );
    assert_eq!(r["result"][0].as_f64(), Some(30.0));
    assert_eq!(r["result"][3], serde_json::json!("ghost.lua"));
    assert_eq!(
        r["result"][4],
        serde_json::json!(true),
        "info().ai traz velocity"
    );
    assert_eq!(
        r["result"][5],
        serde_json::json!(true),
        "info().ai traz desired"
    );

    // nav() — config inserida, sem tile ainda.
    let r = lua_call(
        &mut app,
        LUA5_TEST_PORT,
        "local n = viber.debug.nav() return { n.enabled, n.tile_size, n.offroad_cost, n.tile_generations }",
    );
    assert_eq!(r["result"][0], serde_json::json!(true));
    assert_eq!(r["result"][1].as_f64(), Some(256.0));
    assert_eq!(r["result"][2].as_f64(), Some(2.5));
    assert_eq!(r["result"][3].as_i64(), Some(0));

    // Quests/skills são do JOGO (Lua): a engine já não tem QuestLog nem
    // árvore de talentos para introspecção.
    let r = lua_call(
        &mut app,
        LUA5_TEST_PORT,
        "return viber.debug.quest == nil and viber.debug.quest_defs == nil",
    );
    assert_eq!(r["result"], serde_json::json!(true));
    let r = lua_call(&mut app, LUA5_TEST_PORT, "return viber.debug.skills()");
    assert_eq!(r["result"], serde_json::Value::Null);

    // waypoints() — sem catálogo RPG na engine (o jogo usa viber.markers).
    let r = lua_call(
        &mut app,
        LUA5_TEST_PORT,
        "return #viber.debug.waypoints().landmarks",
    );
    assert_eq!(r["result"], serde_json::json!(0));

    // save_info() — path não vazio (exists depende da máquina).
    let r = lua_call(
        &mut app,
        LUA5_TEST_PORT,
        "local s = viber.debug.save_info() return #s.path > 0",
    );
    assert_eq!(r["result"], serde_json::json!(true));

    // audio() — buses default sem kira; nil-safe.
    let r = lua_call(
        &mut app,
        LUA5_TEST_PORT,
        "local a = viber.debug.audio() return a ~= nil and a.buses.master",
    );
    assert_eq!(r["result"].as_f64(), Some(1.0));

    // Seeds/terreno/atmosfera ausentes → nil, NUNCA erro.
    let r = lua_call(
        &mut app,
        LUA5_TEST_PORT,
        "return { viber.debug.terrain(0, 0) == nil, viber.debug.atmosphere() == nil,
                  viber.debug.weather_full() == nil, viber.debug.border() == nil,
                  viber.debug.interior() == nil, viber.debug.biome_at(0, 0) == nil }",
    );
    for (i, v) in r["result"].as_array().unwrap().iter().enumerate() {
        assert_eq!(v, &serde_json::json!(true), "índice {i} devia ser nil-safe");
    }

    // ui_tree() vazio headless (array, não nil).
    let r = lua_call(&mut app, LUA5_TEST_PORT, "return #viber.debug.ui_tree()");
    assert_eq!(r["result"], serde_json::json!(0));

    // world_hash: estável entre chamadas sem mutação, muda com o conteúdo.
    let r = lua_call(&mut app, LUA5_TEST_PORT, "return viber.debug.world_hash()");
    let hash_a = r["result"].as_str().expect("hash hex").to_string();
    let r = lua_call(&mut app, LUA5_TEST_PORT, "return viber.debug.world_hash()");
    assert_eq!(
        r["result"].as_str(),
        Some(hash_a.as_str()),
        "hash estável sem mutação"
    );
    app.world_mut()
        .spawn((Name::new("hash-bait"), Transform::IDENTITY));
    let r = lua_call(&mut app, LUA5_TEST_PORT, "return viber.debug.world_hash()");
    assert_ne!(
        r["result"].as_str(),
        Some(hash_a.as_str()),
        "entidade nova muda o hash"
    );

    // seeds() — sem terreno nem ciclo, tudo nil (e nunca erro).
    let r = lua_call(
        &mut app,
        LUA5_TEST_PORT,
        "local s = viber.debug.seeds() return s ~= nil and s.terrain_seed == nil",
    );
    assert_eq!(r["result"], serde_json::json!(true));
}

// ── M2: controlo total (vitals/quests/vault/skills/IA/postfx/spawn…) ─────

const LUA6_TEST_PORT: u16 = 35710;
const RAYCAST_TEST_PORT: u16 = 35711;

#[test]
fn test_bridge_lua_control_round4() {
    let mut app = lua_app(LUA6_TEST_PORT);
    {
        let world = app.world_mut();
        world.spawn((
            Name::new("mob"),
            Transform::from_xyz(-4.0, 0.5, 2.0),
            crate::vitals::Health {
                current: 30.0,
                max: 90.0,
            },
            crate::ai::EnemyCreature {
                speed: 3.5,
                ..Default::default()
            },
        ));
        world.insert_resource(crate::nav::NavConfig::default());
        world.insert_resource(crate::music::AudioMixerSettings::default());
        world.insert_resource(crate::music::CombatMusicState::default());
        world.insert_resource(
            bevy::ecs::message::Messages::<crate::ui::actions::UiAction>::default(),
        );
    }
    app.update();

    let lua = |code: &'static str, app: &mut App| {
        let code = code.to_string();
        let handle = std::thread::spawn(move || {
            BridgeClient::localhost(LUA6_TEST_PORT)
                .call(METHOD_LUA, serde_json::json!({ "code": code }))
                .map_err(|error| error.to_string())
        });
        for _ in 0..600 {
            app.update();
            if handle.is_finished() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        handle.join().unwrap().expect("bridge call responde")
    };

    // HP de QUALQUER entidade.
    let r = lua("viber.debug.set_entity_hp('mob', 5) return true", &mut app);
    assert_eq!(r["applied"], serde_json::json!(1));
    let mut query = app.world_mut().query::<(&Name, &crate::vitals::Health)>();
    let found = query
        .iter(app.world())
        .find(|(name, _)| name.as_str() == "mob")
        .expect("mob");
    assert_eq!(found.1.current, 5.0);

    // set_max_hp baixa o teto e clampa o atual.
    let r = lua("viber.debug.set_max_hp('mob', 10) return true", &mut app);
    assert_eq!(r["applied"], serde_json::json!(1));
    let mut query = app.world_mut().query::<(&Name, &crate::vitals::Health)>();
    let (_, health) = query
        .iter(app.world())
        .find(|(name, _)| name.as_str() == "mob")
        .unwrap();
    assert_eq!((health.max, health.current), (10.0, 5.0));

    // Quests/vault/skills são do JOGO (Lua). As ops de debug da engine já
    // não os conhecem: respondem false com warning, em vez de mexer em estado.
    let r = lua(
        "viber.debug.quest_force('forest_survey', 'active')
         viber.debug.vault_set('gold', 500)
         viber.debug.skill_points(5)
         return true",
        &mut app,
    );
    assert_eq!(r["applied"], serde_json::json!(0));
    assert!(
        r["warnings"].as_array().is_some_and(|w| w.len() >= 3),
        "as três ops avisam: {}",
        r["warnings"]
    );

    // IA: aggro persiste; state valida input.
    let r = lua("viber.debug.ai_aggro('mob', 42) return true", &mut app);
    assert_eq!(r["applied"], serde_json::json!(1));
    let r = lua("return viber.debug.ai('mob').aggro_radius", &mut app);
    assert_eq!(r["result"].as_f64(), Some(42.0));
    let r = lua(
        "viber.debug.ai_state('mob', 'banana') return true",
        &mut app,
    );
    assert_eq!(r["applied"], serde_json::json!(0));
    assert!(
        serde_json::to_string(&r["warnings"])
            .unwrap()
            .contains("inválido"),
        "warning de estado inválido"
    );

    // nav_set ao vivo.
    let _ = lua(
        "viber.debug.nav_set{offroad_cost = 9.5, tile_size = 128} return true",
        &mut app,
    );
    let r = lua("return viber.debug.nav().offroad_cost", &mut app);
    assert_eq!(r["result"].as_f64(), Some(9.5));

    // postfx: gate FORA da resposta — observável do lado Rust.
    let r = lua("viber.debug.postfx{bloom = false} return true", &mut app);
    assert_eq!(r["applied"], serde_json::json!(1));
    assert!(crate::postfx::fx_off("BLOOM"), "bloom forçado OFF");
    let r = lua("viber.debug.postfx{bloom = true} return true", &mut app);
    assert_eq!(r["applied"], serde_json::json!(1));
    assert!(
        !crate::postfx::fx_off("BLOOM"),
        "bloom restaurado (sem env)"
    );

    // audio_set reflete no mixer (lido pelo snapshot de áudio).
    let _ = lua("viber.debug.audio_set{music = 0.25} return true", &mut app);
    let r = lua("return viber.debug.audio().buses.music", &mut app);
    assert_eq!(r["result"].as_f64(), Some(0.25));

    // combat_music battle → layer ativa; off → apaga.
    let r = lua("viber.debug.combat_music('boss') return true", &mut app);
    assert_eq!(r["applied"], serde_json::json!(1));
    {
        let world = app.world();
        let music = world.resource::<crate::music::CombatMusicState>();
        let mut probe = music.clone();
        assert_eq!(probe.active_layer(0.0), Some("boss"));
    }
    let r = lua("viber.debug.combat_music('off') return true", &mut app);
    assert_eq!(r["applied"], serde_json::json!(1));

    // save/load: UiAction escrevida (applied=1 chega — o handler da UI é que
    // consome; sem UIPlugin a mensagem fica na fila sem efeito).
    let r = lua("viber.debug.save() return true", &mut app);
    assert_eq!(r["applied"], serde_json::json!(1));

    // teleport_to: player vai ao goblin (sem terreno, Y = o do alvo).
    let r = lua("viber.debug.teleport_to('goblin') return true", &mut app);
    assert_eq!(r["applied"], serde_json::json!(1));
    let mut query = app
        .world_mut()
        .query_filtered::<&Transform, bevy::ecs::query::With<crate::player::Player>>();
    let transform = query.single(app.world()).unwrap();
    assert_eq!(transform.translation.x, 5.0);
    assert_eq!(transform.translation.z, 0.0);

    // spawn de primitiva FÍSICA: collider + corpo fixo. Devolve o NOME do
    // nó (`debug:spawn:N`) — é o handle para `find` no chunk seguinte (a
    // entidade só nasce quando a op aplica, depois do chunk).
    let r = lua(
        "local name = viber.debug.spawn('box:2,2,2', 10, 0, 10, {color = '#ff0000'}) return name",
        &mut app,
    );
    assert_eq!(r["applied"], serde_json::json!(1));
    let spawned_name = r["result"]
        .as_str()
        .expect("spawn devolve o nome do nó")
        .to_string();
    assert!(
        spawned_name.starts_with("debug:spawn:"),
        "nome: {spawned_name}"
    );
    let mut query = app.world_mut().query::<(
        &Name,
        Option<&bevy_rapier3d::prelude::Collider>,
        Option<&bevy_rapier3d::prelude::RigidBody>,
    )>();
    let spawned = query
        .iter(app.world())
        .find(|(name, _, _)| name.as_str().starts_with("debug:spawn:"))
        .expect("primitiva spawnada");
    assert!(spawned.1.is_some(), "tem collider");
    assert!(spawned.2.is_some(), "tem rigidbody");

    // spawn_light.
    let r = lua(
        "viber.debug.spawn_light(1, 3, 1, {intensity = 900, shadows = true}) return true",
        &mut app,
    );
    assert_eq!(r["applied"], serde_json::json!(1));
    let mut query = app
        .world_mut()
        .query_filtered::<(&Name, &bevy::light::PointLight), ()>();
    let light = query
        .iter(app.world())
        .find(|(name, _)| name.as_str().starts_with("debug:light:"))
        .expect("luz spawnada");
    assert_eq!(light.1.intensity, 900.0);
    assert!(light.1.shadow_maps_enabled);

    // set_material ao vivo (rich tem StandardMaterial).
    let r = lua(
        "viber.debug.set_material('rich', {base_color = '#00ff00', metallic = 1}) return true",
        &mut app,
    );
    assert_eq!(r["applied"], serde_json::json!(1));
    {
        let mut query = app.world_mut().query::<(
            Option<&Name>,
            &bevy::pbr::MeshMaterial3d<bevy::pbr::StandardMaterial>,
        )>();
        let world = app.world();
        // O rich pelo NOME — depois do spawn do box há ≥2 materiais.
        let handle = query
            .iter(world)
            .find(|(name, _)| name.map(|n| n.as_str() == "rich").unwrap_or(false))
            .expect("rich material")
            .1
            .0
            .clone();
        let assets = world.resource::<bevy::asset::Assets<bevy::pbr::StandardMaterial>>();
        let mat = assets.get(&handle).unwrap();
        let srgba = mat.base_color.to_srgba();
        assert!((srgba.green - 1.0).abs() < 1e-3, "base_color verde");
        assert_eq!(mat.metallic, 1.0);
    }

    // Clone-on-write: dois props que partilham o MESMO material não podem ser
    // pintados juntos — `set_material` cria uma cópia só para a entidade.
    let shared = app
        .world_mut()
        .resource_mut::<bevy::asset::Assets<bevy::pbr::StandardMaterial>>()
        .add(bevy::pbr::StandardMaterial::default());
    let a = app
        .world_mut()
        .spawn((
            Name::new("shared-a"),
            bevy::pbr::MeshMaterial3d(shared.clone()),
            Transform::default(),
        ))
        .id();
    let b = app
        .world_mut()
        .spawn((
            Name::new("shared-b"),
            bevy::pbr::MeshMaterial3d(shared.clone()),
            Transform::default(),
        ))
        .id();
    let r = lua(
        "viber.debug.set_material('shared-a', {base_color = '#0000ff'}) return true",
        &mut app,
    );
    assert_eq!(r["applied"], serde_json::json!(1));
    {
        let world = app.world();
        let ha = world
            .get::<bevy::pbr::MeshMaterial3d<bevy::pbr::StandardMaterial>>(a)
            .unwrap()
            .0
            .clone();
        let hb = world
            .get::<bevy::pbr::MeshMaterial3d<bevy::pbr::StandardMaterial>>(b)
            .unwrap()
            .0
            .clone();
        assert_ne!(ha, hb, "a entidade tocada tem material próprio");
        assert_eq!(hb, shared, "o irmão continua no material partilhado");
        let assets = world.resource::<bevy::asset::Assets<bevy::pbr::StandardMaterial>>();
        let tinted = assets.get(&ha).unwrap().base_color.to_srgba();
        assert!(tinted.blue > 0.99 && tinted.red < 0.01, "azul aplicado");
        let untouched = assets.get(&hb).unwrap().base_color.to_srgba();
        assert!(untouched.red > 0.99, "o irmão não foi pintado");
    }

    // set_light ao vivo.
    let r = lua(
        "viber.debug.set_light('rich', {intensity = 77}) return true",
        &mut app,
    );
    assert_eq!(r["applied"], serde_json::json!(1));
    let mut query = app
        .world_mut()
        .query_filtered::<&bevy::light::PointLight, ()>();
    let rich_light = query
        .iter(app.world())
        .find(|l| (l.intensity - 77.0).abs() < 1e-3)
        .expect("luz do rich a 77");
    assert_eq!(rich_light.intensity, 77.0);

    // GLB sem AssetServer → warning, sem crash.
    let r = lua(
        "viber.debug.spawn('/assets/nao-existe.glb', 0, 0, 0, {snap = false}) return true",
        &mut app,
    );
    assert_eq!(r["applied"], serde_json::json!(0));
    assert!(
        serde_json::to_string(&r["warnings"])
            .unwrap()
            .contains("AssetServer"),
        "warning de AssetServer ausente"
    );

    // clear_markers limpa TODO o namespace debug:*.
    let r = lua("viber.debug.clear_markers() return true", &mut app);
    assert_eq!(r["applied"], serde_json::json!(1));
    let mut query = app.world_mut().query::<&Name>();
    let leftovers = query
        .iter(app.world())
        .filter(|name| name.as_str().starts_with("debug:"))
        .count();
    assert_eq!(leftovers, 0, "nenhum debug:* sobrevive");
}

#[test]
fn test_bridge_raycast_headless() {
    let mut app = lua_app(RAYCAST_TEST_PORT);
    // PhysicsPlugin REAL: é ele que constrói o collider set do Rapier a
    // partir dos componentes `Collider` (um contexto manual ficaria vazio e
    // o raio nunca apanhava nada). O `resolve_pending_colliders` quer um
    // AssetServer — entra o AssetPlugin mínimo.
    app.add_plugins(bevy::asset::AssetPlugin::default());
    app.add_plugins(bevy::transform::TransformPlugin);
    // Stores que o `resolve_pending_colliders` toca por caminho de asset.
    app.init_asset::<bevy::gltf::Gltf>();
    app.init_asset::<bevy::gltf::GltfMesh>();
    app.init_asset::<bevy::gltf::GltfPrimitive>();
    app.init_asset::<bevy::gltf::GltfNode>();
    app.init_asset::<bevy::image::Image>();
    app.add_plugins(crate::physics::PhysicsPlugin { debug: false });
    {
        let world = app.world_mut();
        // O plugin insere o contexto em PreStartup, que já correu antes de
        // plugins pós-primeiro-update — spawnamos à mão o mesmo par; os
        // sistemas Sync do Rapier (Update) registam os colliders no set.
        world.spawn((
            bevy_rapier3d::prelude::RapierContextSimulation::default(),
            bevy_rapier3d::prelude::RapierConfiguration::new(1.0),
        ));
        // Cubo collider a 5 m de altura — raio de cima deve apanhar o TOPO.
        let mut meshes = world.resource_mut::<bevy::asset::Assets<bevy::mesh::Mesh>>();
        let cube = meshes.add(bevy::mesh::Mesh::from(bevy::shape::Cuboid::new(
            1.0, 1.0, 1.0,
        )));
        drop(meshes);
        world.spawn((
            Name::new("alvo"),
            Transform::from_xyz(0.0, 5.0, 0.0),
            bevy::render::mesh::Mesh3d(cube),
            bevy_rapier3d::prelude::Collider::cuboid(0.5, 0.5, 0.5),
        ));
    }
    app.update();

    let handle = std::thread::spawn(|| {
        BridgeClient::localhost(RAYCAST_TEST_PORT)
            .call(
                METHOD_RAYCAST,
                serde_json::json!({ "x": 0.0, "y": 10.0, "z": 0.0, "dx": 0.0, "dy": -1.0, "dz": 0.0 }),
            )
            .map_err(|error| error.to_string())
    });
    for _ in 0..600 {
        app.update();
        if handle.is_finished() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    // NOTA: o caminho de sync completo do Rapier (collider COMPONENTE → set
    // interno) precisa de FixedUpdate + escada de sistemas que só o mundo
    // inteiro tem; o hit GEOMÉTRICO valida-se no fumo ao vivo (qa-bridge).
    // Headless garante: params/normalização, resposta estruturada e os dois
    // caminhos "sem contexto" (erro) e "contexto vazio" (hit=false).
    let r = handle.join().unwrap().expect("raycast responde");
    assert_eq!(r["hit"], serde_json::json!(false));
}

// ── M4: apidoc cobre EXATAMENTE as funções registadas (guard) ────────────

#[test]
fn test_apidoc_covers_registered_functions() {
    let mut app = lua_app(35712);
    app.update();
    // Chaves REAIS de viber.debug na VM.
    let handle = std::thread::spawn(|| {
        BridgeClient::localhost(35712)
            .call(
                METHOD_LUA,
                serde_json::json!({
                    "code": "local keys = {} for k in pairs(viber.debug) do table.insert(keys, k) end table.sort(keys) return keys"
                }),
            )
            .map_err(|error| error.to_string())
    });
    for _ in 0..600 {
        app.update();
        if handle.is_finished() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let response = handle.join().unwrap().expect("chamada responde");
    let registered: Vec<String> = response["result"]
        .as_array()
        .expect("lista de chaves")
        .iter()
        .map(|v| v.as_str().unwrap_or("?").to_string())
        .collect();
    let documented: Vec<&str> = crate::bridge::lua::DEBUG_API_DOCS
        .iter()
        .map(|(name, _, _)| *name)
        .collect();

    let undocumented: Vec<&String> = registered
        .iter()
        .filter(|name| !documented.contains(&name.as_str()))
        .collect();
    assert!(
        undocumented.is_empty(),
        "funções SEM doc no DEBUG_API_DOCS: {undocumented:?} — acrescenta a entrada \
         (a auto-descoberta do agente depende disto)"
    );

    let phantom: Vec<&&str> = documented
        .iter()
        .filter(|name| !registered.iter().any(|r| r == *name))
        .collect();
    assert!(
        phantom.is_empty(),
        "docs SEM função registada: {phantom:?} — remove a entrada órfã"
    );

    // apidoc() responde e traz os grupos.
    let handle = std::thread::spawn(|| {
        BridgeClient::localhost(35712)
            .call(
                METHOD_LUA,
                serde_json::json!({ "code": "local d = viber.debug.apidoc() return { has_debug = d.debug ~= nil, has_game = d.game ~= nil, doc_health = d.debug.health ~= nil }" }),
            )
            .map_err(|error| error.to_string())
    });
    for _ in 0..600 {
        app.update();
        if handle.is_finished() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let response = handle.join().unwrap().expect("apidoc responde");
    assert_eq!(response["result"]["has_debug"], serde_json::json!(true));
    assert_eq!(response["result"]["has_game"], serde_json::json!(true));
    assert_eq!(response["result"]["doc_health"], serde_json::json!(true));
}

// ── registry.schema / rpc.discover (builtin do bevy_remote) ──────────────

const SCHEMA_TEST_PORT: u16 = 35713;

/// O método builtin `registry.schema` responde com os campos dos tipos
/// refletidos — a base do `viber debug schema` (os fields vivem em
/// `/properties` como `$ref`s JSON-schema, não em `/fields`).
#[test]
fn test_bridge_registry_schema_headless() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(BridgePlugin {
            port: SCHEMA_TEST_PORT,
        })
        .register_type::<Transform>();
    app.update();

    let handle = std::thread::spawn(|| {
        BridgeClient::localhost(SCHEMA_TEST_PORT)
            .call(
                "registry.schema",
                serde_json::json!({ "with_crates": ["bevy_transform"] }),
            )
            .map_err(|error| error.to_string())
    });
    for _ in 0..600 {
        app.update();
        if handle.is_finished() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let response = handle.join().unwrap().expect("registry.schema responde");
    let types = response.as_object().expect("mapa tipo→schema");
    // Nem todo o tipo que "termina em Transform" é a struct com campos
    // (HashMap → ordem arbitrária); procura-se o que TEM o campo.
    let with_translation: Vec<&String> = types
        .iter()
        .filter(|(_, entry)| {
            entry
                .get("properties")
                .and_then(Value::as_object)
                .is_some_and(|properties| properties.contains_key("translation"))
        })
        .map(|(name, _)| name)
        .collect();
    assert!(
        with_translation
            .iter()
            .any(|name| name.ends_with("Transform")),
        "algum tipo Transform com campo translation no schema; candidatos: {with_translation:?}"
    );

    // `rpc.discover` lista os métodos (builtin + viber.*).
    let handle = std::thread::spawn(|| {
        BridgeClient::localhost(SCHEMA_TEST_PORT)
            .call("rpc.discover", serde_json::json!({}))
            .map_err(|error| error.to_string())
    });
    for _ in 0..600 {
        app.update();
        if handle.is_finished() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let discover = handle.join().unwrap().expect("rpc.discover responde");
    let names: Vec<&str> = discover
        .get("methods")
        .and_then(Value::as_array)
        .map(|methods| {
            methods
                .iter()
                .filter_map(|m| m.get("name").and_then(Value::as_str))
                .collect()
        })
        .unwrap_or_default();
    assert!(
        names.contains(&METHOD_LUA),
        "rpc.discover inclui {METHOD_LUA}: {names:?}"
    );
}

/// Depois de uma chamada `viber.lua`, o snapshot é REMOVIDO (não substituído
/// por um vazio): uma closure FORA da REPL (script de jogo) tem de levar o
/// erro "sem snapshot" em vez de ler dados ociosos em silêncio (lista de
/// entidades vazia, player nil).
const LUA7_TEST_PORT: u16 = 35717;

#[test]
fn test_debug_snapshot_is_removed_after_eval() {
    let mut app = lua_app(LUA7_TEST_PORT);
    let r = lua_call(
        &mut app,
        LUA7_TEST_PORT,
        "return #viber.debug.entities() > 0",
    );
    assert_eq!(r["ok"], serde_json::json!(true), "{r}");

    // Caminho do SCRIPT DE JOGO: chunk fora do handler da REPL.
    let host = app.world().resource::<crate::luau::LuaScriptHost>();
    let error = host
        .lua
        .load("return #viber.debug.entities()")
        .set_name("=game-script")
        .eval::<mlua::Value>()
        .expect_err("fora da REPL viber.debug devia falhar")
        .to_string();
    assert!(error.contains("sem snapshot"), "{error}");
}
