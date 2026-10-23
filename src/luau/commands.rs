//! Comandos que scripts enfileiram e a engine aplica pós-frame
//! (`runtime::luau_update` drena [`ScriptCommand`]s de todos os `on_update`).
//! A engine provê os BLOCOS PRIMITIVOS (percepção, movimento com snap no
//! terreno, combate, UI) — a composição de comportamento vive no Luau.

use bevy::prelude::*;
use mlua::Function;

/// Comandos que scripts enfileiram e a engine aplica pós-frame. A engine
/// provê os BLOCOS PRIMITIVOS (percepção, movimento com snap no terreno,
/// combate, UI) — a composição de comportamento vive no Luau.
#[derive(Debug, Clone)]
pub enum ScriptCommand {
    /// Velocidade planar desejada (m/s) para este frame. NÃO é um passo já
    /// multiplicado por `dt`: o consumidor ([`crate::ai::apply_ai_locomotion`])
    /// faz a rampa de aceleração, o passo, o snap de Y e a viragem.
    MoveBy(Entity, Vec2),
    /// Marcha para um PONTO (`viber.move_towards`).
    ///
    /// Separado do [`Self::MoveBy`] porque o destino é informação que o
    /// pathfinding precisa e a velocidade não carrega: projectar a velocidade
    /// para a frente só nomeia pontos sobre a linha recta — que é exactamente
    /// a linha onde está a parede.
    MoveTowards {
        entity: Entity,
        goal: Vec2,
        speed: f32,
    },
    /// Vira a entidade para olhar um ponto (yaw only).
    FaceTowards(Entity, Vec3),
    /// Fixa os nominais de locomoção do rig (m/s) e, opcionalmente, a taxa de
    /// viragem (rad/s). Desliga a auto-calibração.
    SetLocomotion {
        entity: Entity,
        walk: f32,
        run: f32,
        turn_rate: Option<f32>,
    },
    /// `viber.teleport_player(x, y, z [, id_ignorado])` — o `id` opcional é
    /// excluído do raio de aterragem (o próprio marco: um poço/torre não pode
    /// servir de "chão" por cima dele mesmo).
    TeleportPlayer {
        pos: Vec3,
        exclude: Option<Entity>,
    },
    /// `viber.change_scene(id, entry)` — troca de cena (fade, descarrega, carrega).
    /// `via` = nome da entidade que pediu (o `@back` do regresso aponta-a).
    ChangeScene {
        scene: String,
        entry: Option<String>,
        via: Option<String>,
    },
    /// Empurrão do herói (dash/salto lateral) pelo character controller —
    /// colide com paredes, ao contrário do teleporte.
    PushPlayer {
        velocity: Vec3,
    },
    HealPlayer(f32),
    /// `viber.lantern(intensity)` — acende/reforça a lanterna do viajante
    /// (a tocha equipada ilumina mesmo de dia; 0 = só a automática nocturna).
    SetLantern(f32),
    /// `viber.bone_offset(bone, rx, ry, rz)` — rotação ADITIVA num osso do
    /// herói, aplicada por cima de qualquer animação (idle/walk/run).
    /// `Quat::IDENTITY` remove a entrada.
    SetBoneOffset {
        bone: String,
        rotation: Quat,
    },
    /// Dispara um projétil de um `<ProjectileTemplate>` (`viber.fire_projectile`).
    FireProjectile {
        template: String,
        origin: Vec3,
        target: Vec3,
        shooter: Option<Entity>,
    },
    /// Destrutível com queda (`break-style: fall`) — tomba na direção
    /// herói→entidade e despawna no fim.
    Topple {
        entity: Entity,
    },
    /// Reclama uma ação da UI (`viber.own_action("buy")`) — o handler nativo
    /// correspondente cala e a ação chega a Lua via `viber.events()`.
    OwnUiAction(String),
    /// Mensagem de UI (balão/toast) — consumida via [`ScriptToast`].
    Toast(String),
    /// Registra alvo de interação ("[J] Minerar") na entidade.
    SetInteraction {
        entity: Entity,
        label: String,
        key: String,
        range: f32,
    },
    Despawn(Entity),
    /// Clip de ação no rig da entidade (`viber.gesture` / `viber.play_clip`):
    /// fuzzy match do nome, blend ~250 ms — o driver de locomoção recupera o
    /// rig no fim do clip. `speed` escala a reprodução (1.0 = nominal; a
    /// colheita nativa usa 1.4).
    Gesture {
        entity: Entity,
        name: String,
        speed: f32,
        /// `viber.play_clip(..., { lock = true })`: segura a última pose
        /// (morte/cadáver). Sem lock, o driver retoma a locomoção no fim.
        lock: bool,
    },
    /// SFX curto (`viber.sound`) tocado na posição da entidade — consumido
    /// pelo `sfx_player_system` do [`crate::ambient`].
    PlaySfx {
        clip: crate::ambient::SfxClip,
        position: Option<Vec3>,
    },
    /// Reclama um sistema nativo (`viber.own_system("dialogue")`) — o handler
    /// nativo correspondente cala e a lógica passa a ser do script.
    OwnSystem(String),
    /// Devolve um sistema nativo ao handler (`viber.release_system`) — o
    /// inverso de [`Self::OwnSystem`]: o script larga o corpo do herói e o
    /// `player_movement` volta a andar com ele.
    ReleaseSystem(String),
    /// Câmara de jogo conduzida por script (`viber.camera_free`). `false`
    /// devolve a pose ao rig de terceira pessoa no frame seguinte.
    CameraFree(bool),
    /// Pose do modo livre (`viber.camera_pose`): olho + ponto olhado.
    CameraPose {
        pos: Vec3,
        look: Vec3,
    },
    /// Esconde/mostra uma entidade e a subárvore (`viber.set_visible`).
    SetVisible {
        entity: Entity,
        visible: bool,
    },
    /// Dano em ÁREA (`viber.radial_damage`): falloff linear (skills.rs) +
    /// knockback radial opcional; mortes seguem a paridade do
    /// `kill_creature` (corpo, XP, quests, evento `Kill`).
    RadialDamage {
        x: f32,
        z: f32,
        radius: f32,
        damage: f32,
        knockback: f32,
    },
    /// Burst de partículas (`viber.burst`) — preset validado contra
    /// [`crate::particles::PRESET_NAMES`].
    Burst {
        preset: String,
        pos: Vec3,
        count: usize,
    },
    /// Anel de choque (`viber.ring`).
    Ring {
        x: f32,
        z: f32,
        radius: f32,
        color: Option<[f32; 3]>,
    },
    /// Câmara: shake (`viber.shake`), solavanco direcional
    /// (`viber.kick`) e kick de FOV (`viber.fov_kick`).
    Shake(f32),
    CameraKick(Vec3),
    FovKick(f32),
    /// Pulso de pós-processo (`viber.punch`) e hit-stop (`viber.hit_stop`).
    Punch {
        stops: f32,
        bloom: f32,
    },
    HitStop(f32),
    /// Número de dano flutuante (`viber.damage_number`).
    DamageNumber {
        text: String,
        pos: Vec3,
        color: Option<[f32; 3]>,
    },
    /// Balão de diálogo nativo (`viber.say(texto, segundos?)`).
    Say {
        text: String,
        /// 0 = duração por omissão ([`crate::hud::BALLOON_DURATION`]).
        secs: f32,
    },
    /// Pede uma gravação do save (`viber.save()`) — o `options_system` do
    /// save drena o pedido no mesmo frame (mundo `gameplay: none` incluído).
    SaveNow,
    /// Pede um carregamento do save (`viber.load()`).
    LoadNow,
    /// Grava/carrega um slot (`viber.save_slot(n)` / `viber.load_slot(n)`).
    SaveSlot(u32),
    LoadSlot(u32),
    /// Troca o idioma em runtime (`viber.locale.set`).
    SetLocale(String),
    /// Fecha o jogo (`viber.quit`) — pedido limpo ao AppExit.
    Quit,
    /// Vitais GENÉRICAS de entidade (`viber.entity_set_max_hp`): cria o
    /// `Health` se faltar (current = max); numa entidade existente
    /// redimensiona (current clampe ao novo max).
    /// Toca uma cinemática (`viber.cinematic`); `anchor` = âncora `self`.
    PlayCinematic {
        id: String,
        anchor: Option<Entity>,
    },
    /// Escala uniforme da entidade (`viber.set_scale`) — elites maiores, etc.
    SetScale {
        entity: Entity,
        scale: f32,
    },
    EntitySetMaxHp {
        entity: Entity,
        max: f32,
    },
    /// Dano direto em entidade (`viber.entity_damage`) — sem i-frames (esses
    /// são do path do player). Sem `Health` = no-op (warn 1×). HP ≤ 0 emite
    /// [`ScriptGameEvent::Kill`] e NÃO corre o caminho nativo de morte
    /// (cadáver/XP/quests são do melee) — o script decide o que segue.
    EntityDamage {
        entity: Entity,
        amount: f32,
    },
    /// Cura direta em entidade (`viber.entity_heal`), clampe ao max.
    EntityHeal {
        entity: Entity,
        amount: f32,
    },
    /// Empurrão curto numa entidade (`viber.entity_knockback`) — o jogo
    /// decide o golpe; a engine só aplica o impulso físico.
    EntityKnockback {
        entity: Entity,
        dir: Vec3,
        strength: f32,
    },
    /// Acende o flash de impacto na entidade (`viber.hit_flash`) — leitura
    /// visual do golpe, sem dano nem regra de combate.
    HitFlash {
        entity: Entity,
        secs: f32,
    },
    /// Multiplicador de velocidade do herói (`viber.set_player_speed`).
    SetPlayerSpeed(f32),
    /// Marca a entidade como cadáver (`viber.corpse`): o jogo decide a morte;
    /// a engine só deixa de a animar e, se tiver `RagdollDeath`
    /// (`ragdoll="true"`), começa o ragdoll. Não despawna — isso é do jogo.
    Corpse {
        entity: Entity,
    },
    /// Anexa um GLB a um osso (`viber.attach`) ou remove o slot
    /// (`url = None`, `viber.detach`). Genérico: a engine não sabe o que é
    /// uma arma; só encontra o osso e pendura o prop.
    Attach {
        owner: Option<Entity>,
        slot: String,
        url: Option<String>,
        bone: String,
        pos: Vec3,
        rot: Vec3,
        scale: f32,
    },
    /// Prompt de interação de MUNDO (`viber.prompt(key, label)`): um
    /// controller publica o verbo do alvo atual sem ser ele a entidade
    /// interagida. `label` vazio limpa. É UI genérica — o jogo decide quando.
    Prompt {
        key: String,
        label: String,
    },
    /// Alteração de acessibilidade (`viber.a11y_set`): campos `None` ficam.
    A11ySet {
        subtitle_scale: Option<f32>,
        shake: Option<bool>,
        hitstop: Option<bool>,
        interact_hold: Option<bool>,
    },
    /// Edição VIVA do terreno (`viber.terrain.lower/raise/flatten/crater`):
    /// vai para a [`crate::terrain::delta::TerrainEditQueue`], aplicada pelo
    /// `apply_terrain_edits` no máximo `EDITS_PER_FRAME` por frame; o
    /// rebuild das colunas afetadas (mesh + collider) segue pelo caminho
    /// staged do plugin de LOD.
    TerrainEdit {
        edit: crate::terrain::delta::TerrainEdit,
    },
    /// Instancia um `<Prototype>` do mundo (`viber.spawn_prototype`) — o
    /// spawn REAL é exclusivo (`recipes::spawn::apply_script_spawns`), pós-
    /// frame, com callback `(bits)`.
    SpawnPrototype {
        name: String,
        pos: Vec3,
        /// Sem cota explícita: assenta o Y na superfície renderizada.
        seat: bool,
        on_spawned: Option<Function>,
        caller_path: Option<String>,
    },
    /// Pose local da entidade (`viber.set_pose`). O yaw é `Quat::from_rotation_y`.
    /// A escala fica. Com `own_system("locomotion")` o `player_movement` não
    /// disputa esta escrita no herói.
    SetPose {
        entity: Entity,
        pos: Vec3,
        yaw: f32,
    },
    /// Loop de SFX (`viber.sound_loop`). `clip` é nome do registry ou caminho
    /// relativo ao `sfx_dir` (`vehicles/engine_rev.ogg`).
    SoundLoop {
        id: String,
        clip: String,
        pitch: f64,
        volume: f32,
        position: Option<Vec3>,
        /// Entidade cujo script pediu o loop (o loop morre com ela). `None`
        /// em chamadas fora de `on_update` (tick/REPL).
        owner: Option<Entity>,
    },
    /// Pára um loop (`viber.sound_stop`).
    SoundStop(String),
    /// Força a layer de BGM (`viber.music.layer`). `None` devolve o automático.
    MusicLayer(Option<String>),
    /// Liga/desliga a música de combate (`viber.music.combat`). `boss` promove
    /// para a layer `boss`; `active=false` corta já (sem esperar o hold).
    MusicCombat {
        active: bool,
        boss: bool,
    },
    /// Duck temporário de um bus (`viber.audio.duck`): `gain` linear durante
    /// `secs` segundos. Expira sozinho.
    AudioDuck {
        bus: String,
        gain: f32,
        secs: f32,
    },
    /// Reverb dos buses de SFX/voz (`viber.audio.reverb`).
    AudioReverb {
        preset: String,
        wet: Option<f32>,
    },
    /// Decal de impacto (`viber.decal`): quad projectado com fade.
    Decal {
        url: String,
        position: Vec3,
        normal: Vec3,
        size: f32,
        life: f32,
        color: [f32; 3],
    },
    /// Marcas de HUD (`viber.markers`): blips `(pos, tower)`, faróis e waypoint.
    SetMarkers {
        blips: Vec<(Vec3, bool)>,
        beacons: Vec<Vec3>,
        waypoint: Option<(Vec3, String)>,
    },
    /// `viber.noise(loudness)` — audição das criaturas sem script.
    Noise {
        pos: Vec3,
        loudness: f32,
    },
}
