//! BGM driver: loops every `<MusicLayer>` at volume 0 and crossfades the
//! layer matching the player's zone (ported from the example's `bgmZone`).
//!
//! **Backend kira** ([`bevy_kira_audio`]): dois buses tipados
//! ([`MusicBus`]/[`SfxBus`]) recebem os volumes do [`AudioMixerSettings`] via
//! [`mixer_sync`] — um som NOVO não precisa de saber do mixer, o bus aplica-o
//! a tudo o que está a tocar (incl. one-shots a meio). O crossfade
//! ([`fade_step`]) continua linear em espaço LINEAR (puro + testado); a
//! conversão para dB acontece só na fronteira ([`linear_to_db`]).

use std::time::Duration;

use bevy::prelude::*;
use bevy_kira_audio::prelude::{Mix, ReverbHandle};
use bevy_kira_audio::{AudioChannel, AudioControl, AudioInstance, AudioTween};

/// Piso de silêncio em dB — kira trata ≤ −60 dB como inaudível.
const DB_SILENCE: f32 = -60.0;

/// Linear 0..1 → decibéis (kira trabalha em dB; −60 = silêncio).
pub fn linear_to_db(v: f32) -> f32 {
    if !v.is_finite() || v <= 0.000_1 {
        DB_SILENCE
    } else {
        (20.0 * v.log10()).max(DB_SILENCE)
    }
}

/// Bus de música (`<AudioMixer music>` × master) — BGM e loops de ambiente
/// musicais? NÃO: só BGM ([`MusicLayerTag`]). Ambiente de água/chuva vai no
/// bus SFX, como os efeitos.
#[derive(Resource)]
pub struct MusicBus;

/// Bus de efeitos (`<AudioMixer sfx>` × master) — SFX one-shots + loops de
/// água/chuva.
#[derive(Resource)]
pub struct SfxBus;

/// Bus de VOZ (`<AudioMixer voice>` × master) — falas, barks e grunhidos.
/// Separado do SFX para o ducking de diálogo/cinemática poder baixar música e
/// efeitos sem calar a voz.
#[derive(Resource)]
pub struct VoiceBus;

/// Bus de AMBIENTE (`<AudioMixer ambience>` × master) — vento, chuva, água,
/// fogo. Separado do SFX para o jogador poder baixar o ambiente sem perder
/// os sons de combate.
#[derive(Resource)]
pub struct AmbienceBus;

/// Parâmetros de um loop de SFX pedido por script (`viber.sound_loop`).
#[derive(Clone, Debug)]
pub struct ScriptLoopParams {
    pub clip: String,
    pub pitch: f64,
    pub volume: f32,
    pub position: Option<Vec3>,
    /// Entidade que pediu o loop; `None` = world-scoped (REPL/tick).
    pub owner: Option<bevy::prelude::Entity>,
}

/// Loops que os scripts querem vivos neste frame. O driver de áudio lê
/// `wanted` e reconcilia as instâncias; sem o recurso o comando é no-op.
#[derive(Resource, Default)]
pub struct ScriptSoundLoops {
    pub wanted: std::collections::HashMap<String, ScriptLoopParams>,
}

/// Layer de BGM forçada por script (`viber.music.layer`). `None` = automático.
#[derive(Resource, Default)]
pub struct MusicLayerOverride(pub Option<String>);

/// Bus volumes from `<AudioMixer master music sfx>`.
///
/// The resource always exists (worlds without an `<AudioMixer>` get this
/// default): a derived `Default` would zero every bus — i.e. a silent game —
/// and any system taking `ResMut<AudioMixerSettings>` used to panic outright
/// on a world that never declared the tag.
#[derive(Debug, Clone, Resource)]
pub struct AudioMixerSettings {
    pub master: f32,
    pub music: f32,
    pub sfx: f32,
    /// Falas/barks (`<AudioMixer voice>`).
    pub voice: f32,
    /// Ambiente/loops de mundo (`<AudioMixer ambience>`).
    pub ambience: f32,
}

impl Default for AudioMixerSettings {
    fn default() -> Self {
        Self {
            master: 1.0,
            music: 1.0,
            sfx: 1.0,
            voice: 1.0,
            ambience: 1.0,
        }
    }
}

/// Nome de bus aceite por `viber.audio.duck`/`reverb` e pelos saves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioBus {
    Master,
    Music,
    Sfx,
    Voice,
    Ambience,
}

impl AudioBus {
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "master" => Some(Self::Master),
            "music" => Some(Self::Music),
            "sfx" => Some(Self::Sfx),
            "voice" => Some(Self::Voice),
            "ambience" | "ambient" => Some(Self::Ambience),
            _ => None,
        }
    }
}

/// Um duck temporário: `gain` linear e o instante (elapsed_secs) em que expira.
///
/// `stack` guarda os pedidos ATIVOS: ducks sobrepostos (diálogo + cinemática)
/// não se cancelam — o gain efetivo é o MENOR e cada um expira por si. Com um
/// único slot, o pedido mais recente substituía o anterior (o diálogo subia a
/// meio da fala) ou, com `min`, ficava preso até ao fim do mais longo.
#[derive(Debug, Clone)]
pub struct DuckLevel {
    pub gain: f32,
    pub until: f64,
    stack: Vec<(f32, f64)>,
}

impl Default for DuckLevel {
    fn default() -> Self {
        Self {
            // 1.0 e não o 0.0 derivado: um bus sem ducks não pode nascer mudo.
            gain: 1.0,
            until: 0.0,
            stack: Vec::new(),
        }
    }
}

impl DuckLevel {
    pub fn active(&self, now: f64) -> f32 {
        self.stack
            .iter()
            .filter(|(_, until)| now < *until)
            .map(|(gain, _)| gain.clamp(0.0, 1.0))
            .fold(1.0, f32::min)
    }
}

/// Ducking por bus, pedido por scripts (`viber.audio.duck`). O
/// [`mixer_sync`] multiplica o volume do bus por este fator; expira sozinho.
#[derive(Debug, Clone, Resource, Default)]
pub struct AudioDuck {
    pub master: DuckLevel,
    pub music: DuckLevel,
    pub sfx: DuckLevel,
    pub voice: DuckLevel,
    pub ambience: DuckLevel,
}

impl AudioDuck {
    pub fn level(&mut self, bus: AudioBus) -> &mut DuckLevel {
        match bus {
            AudioBus::Master => &mut self.master,
            AudioBus::Music => &mut self.music,
            AudioBus::Sfx => &mut self.sfx,
            AudioBus::Voice => &mut self.voice,
            AudioBus::Ambience => &mut self.ambience,
        }
    }

    pub fn apply(&mut self, bus: AudioBus, gain: f32, secs: f32, now: f64) {
        if !gain.is_finite() || !secs.is_finite() || secs <= 0.0 {
            return;
        }
        let level = self.level(bus);
        let requested = gain.clamp(0.0, 1.0);
        let until = now + secs as f64;
        level.stack.retain(|(_, until)| now < *until);
        level.stack.push((requested, until));
        level.gain = level
            .stack
            .iter()
            .map(|(gain, _)| *gain)
            .fold(1.0, f32::min);
        level.until = level
            .stack
            .iter()
            .map(|(_, until)| *until)
            .fold(0.0, f64::max);
    }

    pub fn ducked(&self, bus: AudioBus, now: f64) -> f32 {
        match bus {
            AudioBus::Master => self.master.active(now),
            AudioBus::Music => self.music.active(now),
            AudioBus::Sfx => self.sfx.active(now),
            AudioBus::Voice => self.voice.active(now),
            AudioBus::Ambience => self.ambience.active(now),
        }
    }
}

/// Presets de reverb genéricos (a engine não conhece "gruta" nem "capela";
/// o jogo escolhe o preset e a quantidade de molhado).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ReverbPreset {
    #[default]
    Off,
    Room,
    Hall,
    Cave,
}

impl ReverbPreset {
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "off" => Some(Self::Off),
            "room" => Some(Self::Room),
            "hall" => Some(Self::Hall),
            "cave" => Some(Self::Cave),
            _ => None,
        }
    }

    /// `(feedback, damping, mix)` do kira.
    pub fn params(self) -> (f64, f64, f64) {
        match self {
            Self::Off => (0.0, 0.1, 0.0),
            Self::Room => (0.72, 0.45, 0.22),
            Self::Hall => (0.88, 0.30, 0.38),
            Self::Cave => (0.93, 0.16, 0.55),
        }
    }
}

/// Pedido de reverb (`viber.audio.reverb`). `wet: None` = o mix do preset.
#[derive(Debug, Clone, Copy, Resource, Default)]
pub struct AudioReverb {
    pub preset: ReverbPreset,
    pub wet: Option<f32>,
}

/// Handles dos reverbs criados nos tracks de SFX/voz. Sem kira (apps de
/// teste) ficam `None` e o pedido é ignorado.
#[derive(Resource, Default)]
pub struct ReverbHandles {
    pub sfx: Option<ReverbHandle>,
    pub voice: Option<ReverbHandle>,
}

/// Empurra o preset para os reverbs dos buses. Só mexe quando o pedido muda
/// (o handle do kira não lê o valor de volta; o `Local` é a cache).
pub fn apply_reverb(
    request: Option<Res<AudioReverb>>,
    mut handles: ResMut<ReverbHandles>,
    mut last: Local<Option<(ReverbPreset, Option<u32>)>>,
) {
    let Some(request) = request else {
        return;
    };
    let signature = (request.preset, request.wet.map(f32::to_bits));
    if *last == Some(signature) {
        return;
    }
    *last = Some(signature);
    let (feedback, damping, mix) = request.preset.params();
    let mix = request.wet.unwrap_or(mix as f32).clamp(0.0, 1.0);
    let tween = || AudioTween::linear(Duration::from_millis(180));
    if let Some(handle) = handles.sfx.as_mut() {
        handle.set_feedback(feedback, tween());
        handle.set_damping(damping, tween());
        handle.set_mix(Mix(mix), tween());
    }
    if let Some(handle) = handles.voice.as_mut() {
        handle.set_feedback(feedback, tween());
        handle.set_damping(damping, tween());
        handle.set_mix(Mix(mix), tween());
    }
}

/// One playing BGM layer.
#[derive(Debug, Component)]
pub struct MusicLayerTag {
    pub layer: String,
    pub base_volume: f32,
}

/// Loop à espera que o bus kira arranque o playback (BGM OU ambiente). O
/// [`audio_loop_starter`], consome-o, toca o asset em LOOP e insere
/// [`LoopInstance`]. O asset a carregar não é problema — o bevy_kira_audio
/// começa a tocar quando o `.ogg` chega.
#[derive(Debug, Component)]
pub struct AudioLoopPending {
    /// URL do asset de áudio (convenções `assets/audio/bgm/*.ogg` /
    /// `assets/audio/sfx/**`).
    pub url: String,
    /// `true` = bus de música; `false` = bus de SFX.
    pub music: bool,
}

/// Marker: o [`AudioLoopPending`] desta entidade toca no [`AmbienceBus`]
/// (chuva, água, cachoeiras), não no bus de SFX. Sem isto o `<AudioMixer
/// ambience>` e o duck `ambience` não tinham efeito nenhum — os loops de
/// ambiente iam todos para o SFX.
#[derive(Debug, Component)]
pub struct AmbienceLoop;

/// Instância kira de um loop em curso (handle para o `Assets<AudioInstance>`).
#[derive(Debug, Component)]
pub struct LoopInstance(pub Handle<AudioInstance>);

/// Volume linear corrente de um loop — cache para o [`fade_step`] (o kira
/// não lê o volume de volta).
#[derive(Debug, Component, Default)]
pub struct LoopVolume(pub f32);

/// Player context marker for the driver (reuses the `Player` component).
pub fn bgm_zone(
    x: f32,
    z: f32,
    scene: Option<&crate::worldsys::InteriorSceneConfig>,
) -> &'static str {
    // Bolsa de interior DECLARADA (fora da área do mapa). A zona de masmorra
    // seguia uma caixa copiada à mão (770..950 × 205..355) que ficava para
    // trás de cada vez que a grelha de interiores se movia — e a grelha mudou
    // de sítio: agora segue o `<InteriorScene>`, que é a fonte de verdade.
    if scene.is_some_and(|s| s.contains(x, z)) {
        return "dungeon";
    }
    // Cunha dos Picos Gelados: z <= -240, |x| abre 240 → 1040
    if z <= -240.0 && x.abs() <= 240.0 + 0.0f32.max(-z - 240.0) {
        return "mountain";
    }
    // Vila murada na origem (SpawnExclusion r52 + margem)
    if x * x + z * z < 55.0 * 55.0 {
        return "village";
    }
    "explore"
}

/// Zona de BGM preferida por uma cena que declara uma layer com o PRÓPRIO id
/// (o menu declara `<MusicLayer layer="menu">`). `None` = cena principal ou
/// cena sem layer homónima: aí manda a zona de posição do mundo aberto.
pub fn scene_zone<'a>(
    scene_id: Option<&'a str>,
    has_layer: impl Fn(&str) -> bool,
) -> Option<&'a str> {
    scene_id
        .filter(|id| *id != crate::project::MAIN_SCENE)
        .filter(|id| has_layer(id))
}

/// Segundos que a layer de combate sobrevive ao último evento de combate —
/// tiros trocados mantêm o timer vivo; afastar-se sem luta deixa voltar a
/// música da zona.
pub const COMBAT_MUSIC_HOLD: f32 = 8.0;

/// Estado da música de combate: enquanto ativo, a layer `battle` (ou `boss`
/// quando o alvo o é) ganha à zona do player. Escrito pelos gatilhos de
/// combate (aggro da IA, golpes); expira sozinho após
/// [`COMBAT_MUSIC_HOLD`] s sem eventos.
///
/// O recurso é inicializado no [`crate::ai::AiPlugin`] (que também o
/// escreve) — worlds sem IA ficam com o crossfade puro por zona.
#[derive(Debug, Clone, Resource, Default)]
pub struct CombatMusicState {
    /// Instante (`Time.elapsed_secs_f64()`) até ao qual o combate "soa".
    until: f64,
    /// Alvo atual é um boss → layer `boss` em vez de `battle`.
    boss: bool,
}

impl CombatMusicState {
    /// (Re)acende a música de combate; `boss` promove para a layer de boss
    /// (nunca desce de boss para battle enquanto o timer viver).
    pub fn engage(&mut self, now: f64, boss: bool) {
        // Se o hold anterior JÁ expirou, este engagement define a layer; sem
        // isto um `boss` velho (que só `active_layer` limpa) contaminava o
        // combate normal durante os 8 s seguintes.
        let expired = now >= self.until;
        self.until = (now + COMBAT_MUSIC_HOLD as f64).max(self.until);
        if expired {
            self.boss = boss;
        } else if boss {
            self.boss = true;
        }
    }

    /// Layer de combate ativa agora, expirando o timer (`boss` > `battle`).
    pub fn active_layer(&mut self, now: f64) -> Option<&'static str> {
        if now >= self.until {
            self.boss = false;
            None
        } else if self.boss {
            Some("boss")
        } else {
            Some("battle")
        }
    }

    /// Apaga a música de combate JÁ (debug bridge: `combat_music("off")`) —
    /// sem isto o A/B de BGM esperava os 8 s de `COMBAT_MUSIC_HOLD`.
    pub fn clear(&mut self) {
        self.until = 0.0;
        self.boss = false;
    }
}

/// Target linear volume of one layer given the active zone — SEM os buses
/// (music×master agora vivem no [`MusicBus`], aplicados pelo kira a tudo o
/// que está a tocar).
pub fn layer_target(layer: &str, zone: &str, base_volume: f32) -> f32 {
    if layer == zone {
        // `base-volume` é linear 0..1; 2.0 dava +6 dB (clipping) e um valor
        // negativo virava DB_SILENCE permanente.
        base_volume.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// Crossfade one step: move `current` toward `target` by `speed` per second.
pub fn fade_step(current: f32, target: f32, dt: f32, speed: f32) -> f32 {
    // NaN/inf não morrem no clamp: devolver o `current` evita que
    // `next != current` seja sempre verdadeiro (reescrita de Volume todos
    // os frames para sempre).
    if !current.is_finite() || !target.is_finite() {
        return current;
    }
    let delta = target - current;
    let max_step = speed * dt;
    if delta.abs() <= max_step {
        target
    } else {
        current + delta.signum() * max_step
    }
}

/// Arranca os loops pendentes ([`AudioLoopPending`]) no bus certo e insere
/// [`LoopInstance`]/[`LoopVolume`]. Sem kira vivo (apps de teste headless)
/// os pendentes ficam — os drivers ignoram-nos.
pub fn audio_loop_starter(
    mut commands: Commands,
    pending: Query<(Entity, &AudioLoopPending, Option<&AmbienceLoop>), Added<AudioLoopPending>>,
    server: Res<AssetServer>,
    music: Option<Res<AudioChannel<MusicBus>>>,
    sfx: Option<Res<AudioChannel<SfxBus>>>,
    ambience: Option<Res<AudioChannel<AmbienceBus>>>,
) {
    for (entity, pend, on_ambience) in &pending {
        // Os buses são tipos distintos e o trait de playback não é
        // dyn-compatible — o branch repete 2 linhas, mas mantém o resto único.
        let handle = if pend.music {
            let Some(channel) = music.as_deref() else {
                continue;
            };
            channel
                .play(server.load(&pend.url))
                .looped()
                .with_volume(DB_SILENCE)
                .handle()
        } else if on_ambience.is_some() {
            let Some(channel) = ambience.as_deref() else {
                continue;
            };
            channel
                .play(server.load(&pend.url))
                .looped()
                .with_volume(DB_SILENCE)
                .handle()
        } else {
            let Some(channel) = sfx.as_deref() else {
                continue;
            };
            channel
                .play(server.load(&pend.url))
                .looped()
                .with_volume(DB_SILENCE)
                .handle()
        };
        commands
            .entity(entity)
            .insert((LoopInstance(handle), LoopVolume(0.0)))
            .remove::<AudioLoopPending>();
    }
}

/// Empurra [`AudioMixerSettings`] para os buses kira — sliders do menu e
/// volumes do save afetam INSTANTANEamente o que está a tocar (o modelo
/// antigo multiplicava no momento do spawn; um one-shot a meio não ouvia a
/// mudança). Tween curto = transições sem clique.
pub fn mixer_sync(
    time: Res<Time>,
    mixer: Option<Res<AudioMixerSettings>>,
    duck: Option<Res<AudioDuck>>,
    music: Option<Res<AudioChannel<MusicBus>>>,
    sfx: Option<Res<AudioChannel<SfxBus>>>,
    voice: Option<Res<AudioChannel<VoiceBus>>>,
    ambience: Option<Res<AudioChannel<AmbienceBus>>>,
    mut seeded: Local<bool>,
    mut last: Local<[f32; 5]>,
) {
    let Some(mixer) = mixer else {
        return;
    };
    // 1.º frame aplica SEMPRE (o `last` a zeros é lixo — um mixer autoral
    // 0/0/0 no arranque teria de empurrar −60 dB para os buses).
    let now = time.elapsed_secs_f64();
    let d = duck.as_deref().cloned().unwrap_or_default();
    let values = [
        mixer.master,
        mixer.music,
        mixer.sfx,
        mixer.voice,
        mixer.ambience,
    ];
    let master = (values[0].clamp(0.0, 1.0)) * d.ducked(AudioBus::Master, now);
    // Os fatores de duck mudam sem o mixer mudar; incluí-los na assinatura
    // evita que um duck actue só no frame seguinte ao próximo slider.
    let signature = [
        master,
        values[1].clamp(0.0, 1.0) * d.ducked(AudioBus::Music, now) * master,
        values[2].clamp(0.0, 1.0) * d.ducked(AudioBus::Sfx, now) * master,
        values[3].clamp(0.0, 1.0) * d.ducked(AudioBus::Voice, now) * master,
        values[4].clamp(0.0, 1.0) * d.ducked(AudioBus::Ambience, now) * master,
    ];
    if *seeded && *last == signature {
        return;
    }
    *seeded = true;
    *last = signature;
    let tween_ms = || Duration::from_millis(120);
    if let Some(channel) = music {
        channel
            .set_volume(linear_to_db(signature[1]))
            .linear_fade_in(tween_ms());
    }
    if let Some(channel) = sfx {
        channel
            .set_volume(linear_to_db(signature[2]))
            .linear_fade_in(tween_ms());
    }
    if let Some(channel) = voice {
        channel
            .set_volume(linear_to_db(signature[3]))
            .linear_fade_in(tween_ms());
    }
    if let Some(channel) = ambience {
        channel
            .set_volume(linear_to_db(signature[4]))
            .linear_fade_in(tween_ms());
    }
}

#[derive(Debug, Default, Resource)]
pub struct ScriptLoopPlayback {
    live: std::collections::HashMap<String, (String, Handle<AudioInstance>)>,
}

/// Move each layer's volume toward its zone target every frame.
pub fn music_driver(
    time: Res<Time>,
    players: Query<&GlobalTransform, With<crate::player::Player>>,
    scene: Option<Res<crate::worldsys::InteriorSceneConfig>>,
    current_scene: Option<Res<crate::scene_loader::CurrentScene>>,
    mut combat: ResMut<CombatMusicState>,
    forced: Option<Res<MusicLayerOverride>>,
    mut layers: Query<(&MusicLayerTag, &LoopInstance, &mut LoopVolume)>,
    mut instances: ResMut<Assets<AudioInstance>>,
) {
    // `iter().next()` e não `single()`: mundos com 0 ou ≥2 players (o
    // auto-orbit suporta mundo sem player) ficavam para sempre sem BGM.
    let Some(player) = players.iter().next() else {
        return;
    };
    let pos = player.translation();
    // Uma cena que declara a SUA layer (o menu declara `layer="menu"`) não se
    // guia pela zona de posição do mundo aberto: o acampamento do menu fica a
    // <55 m da origem, o `bgm_zone` dizia "village" e a layer `menu` nunca
    // subia — menu em silêncio. Só vale quando a layer existe na cena.
    let scene_zone = scene_zone(
        current_scene
            .as_deref()
            .and_then(|current| current.id.as_deref()),
        |id| layers.iter().any(|(tag, _, _)| tag.layer == id),
    );
    // Script (`viber.music.layer`) ganha à zona e ao combate. Sem override,
    // combate ativo ganha à zona (boss > battle).
    let auto = combat
        .active_layer(time.elapsed_secs_f64())
        .map(str::to_owned)
        .or_else(|| scene_zone.map(str::to_owned))
        .unwrap_or_else(|| bgm_zone(pos.x, pos.z, scene.as_deref()).to_string());
    let zone = forced
        .as_ref()
        .and_then(|over| over.0.clone())
        .unwrap_or(auto);
    let dt = time.delta_secs().clamp(0.0, 0.2);
    for (tag, instance, mut volume) in &mut layers {
        let target = layer_target(&tag.layer, &zone, tag.base_volume);
        let next = fade_step(volume.0, target, dt, 0.6);
        if next != volume.0 {
            let Some(mut snd) = instances.get_mut(&instance.0) else {
                continue;
            };
            snd.set_decibels(
                linear_to_db(next),
                AudioTween::linear(Duration::from_millis(60)),
            );
            volume.0 = next;
        }
    }
}

/// Resolve `clip` de `viber.sound_loop`: nome do registry ou caminho relativo
/// ao `sfx_dir`.
fn resolve_loop_clip(clip: &str, config: &crate::config::GameConfig) -> String {
    if let Some(known) = crate::luau::sfx::sfx_clip_from_str(clip) {
        config.sfx_path(known.file())
    } else {
        config.sfx_path(clip)
    }
}

/// Mantém os loops pedidos por scripts (pitch, volume, atenuação).
pub fn script_sound_loops(
    loops: Res<ScriptSoundLoops>,
    mut playback: Local<ScriptLoopPlayback>,
    server: Res<AssetServer>,
    config: Option<Res<crate::config::GameConfig>>,
    channel: Option<Res<AudioChannel<SfxBus>>>,
    mut instances: ResMut<Assets<AudioInstance>>,
    editor: Query<&GlobalTransform, With<crate::editor_camera::EditorCamera>>,
    listeners: Query<&GlobalTransform, With<Camera3d>>,
) {
    let (Some(channel), Some(config)) = (channel, config) else {
        return;
    };
    let stale: Vec<String> = playback
        .live
        .keys()
        .filter(|id| !loops.wanted.contains_key(*id))
        .cloned()
        .collect();
    for id in stale {
        if let Some((_, handle)) = playback.live.remove(&id)
            && let Some(mut snd) = instances.get_mut(&handle)
        {
            snd.stop(AudioTween::linear(Duration::from_millis(80)));
        }
    }
    let ear =
        crate::editor_camera::active_camera_eye(&editor, &listeners).map(|cam| cam.translation());
    for (id, params) in &loops.wanted {
        let url = resolve_loop_clip(&params.clip, &config);
        let restart = match playback.live.get(id) {
            Some((clip, _)) => clip != &url,
            None => true,
        };
        if restart {
            if let Some((_, handle)) = playback.live.remove(id)
                && let Some(mut snd) = instances.get_mut(&handle)
            {
                snd.stop(AudioTween::linear(Duration::from_millis(40)));
            }
            let handle = channel
                .play(server.load(url.clone()))
                .looped()
                .with_volume(DB_SILENCE)
                .handle();
            playback.live.insert(id.clone(), (url, handle));
        }
        let Some((_, handle)) = playback.live.get(id) else {
            continue;
        };
        let Some(mut snd) = instances.get_mut(handle) else {
            continue;
        };
        let atten = match (params.position, ear) {
            (Some(pos), Some(ear)) => (1.0 - pos.distance(ear) / 40.0).clamp(0.0, 1.0),
            _ => 1.0,
        };
        snd.set_playback_rate(
            params.pitch.max(0.05),
            AudioTween::linear(Duration::from_millis(90)),
        );
        snd.set_decibels(
            linear_to_db((params.volume * atten).clamp(0.0, 1.0)),
            AudioTween::linear(Duration::from_millis(90)),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_linear_to_db_floor_and_passthrough() {
        assert_eq!(linear_to_db(0.0), DB_SILENCE);
        assert_eq!(linear_to_db(f32::NAN), DB_SILENCE);
        assert_eq!(linear_to_db(-1.0), DB_SILENCE);
        // 1.0 linear = 0 dB.
        assert!((linear_to_db(1.0)).abs() < 1e-4);
        // 0.1 linear ≈ −20 dB.
        assert!((linear_to_db(0.1) + 20.0).abs() < 0.01);
    }

    #[test]
    fn test_bgm_zone_village_at_origin() {
        assert_eq!(bgm_zone(0.0, 0.0, None), "village");
        assert_eq!(bgm_zone(40.0, 30.0, None), "village");
    }

    #[test]
    fn test_bgm_zone_dungeon_box() {
        let scene = crate::worldsys::InteriorSceneConfig {
            min: [2500.0, 2500.0],
            max: [2900.0, 2830.0],
            ..crate::worldsys::InteriorSceneConfig::default()
        };
        assert_eq!(bgm_zone(2700.0, 2700.0, Some(&scene)), "dungeon");
        assert_eq!(bgm_zone(2510.0, 2820.0, Some(&scene)), "dungeon");
        // fora da bolsa (e sem bolsa declarada) cai na zona normal
        assert_eq!(bgm_zone(2400.0, 2700.0, Some(&scene)), "explore");
        assert_eq!(bgm_zone(2700.0, 2700.0, None), "explore");
    }

    #[test]
    fn test_bgm_zone_mountain_wedge() {
        assert_eq!(bgm_zone(0.0, -300.0, None), "mountain");
        assert_eq!(bgm_zone(200.0, -400.0, None), "mountain");
        // wedge fecha a sul: em z=-250 só |x| <= 240+10
        assert_eq!(bgm_zone(600.0, -250.0, None), "explore");
    }

    #[test]
    fn test_bgm_zone_explore_default() {
        assert_eq!(bgm_zone(300.0, 300.0, None), "explore");
    }

    /// O menu declara `layer="menu"` mas o acampamento cai na zona `village`:
    /// a cena tem de ganhar à posição (senão o BGM do menu nunca toca).
    #[test]
    fn test_scene_layer_wins_over_position_zone() {
        let has_menu = |layer: &str| layer == "menu";
        assert_eq!(scene_zone(Some("menu"), has_menu), Some("menu"));
        // A cena principal segue a zona do mapa.
        assert_eq!(scene_zone(Some("main"), has_menu), None);
        // Cena sem layer homónima continua na zona de posição.
        assert_eq!(scene_zone(Some("interiors/chapel"), has_menu), None);
        assert_eq!(scene_zone(None, has_menu), None);
    }

    #[test]
    fn test_layer_target_crossfades() {
        assert!(
            layer_target("explore", "explore", 0.18) > 0.1,
            "active layer gets volume"
        );
        assert_eq!(
            layer_target("boss", "explore", 0.24),
            0.0,
            "inactive layer mutes"
        );
    }

    #[test]
    fn test_fade_step_converges() {
        assert_eq!(fade_step(0.5, 0.5, 0.1, 0.6), 0.5);
        assert!(fade_step(0.0, 1.0, 0.1, 0.6) > 0.0);
        assert_eq!(fade_step(0.0, 1.0, 5.0, 0.6), 1.0, "reaches target");
        // NaN não se propaga: devolve o current (o driver não reescreve).
        assert!(fade_step(f32::NAN, 1.0, 0.1, 0.6).is_nan());
        assert_eq!(fade_step(0.25, f32::NAN, 0.1, 0.6), 0.25);
    }

    #[test]
    fn test_combat_music_engage_and_expiry() {
        let mut combat = CombatMusicState::default();
        assert_eq!(combat.active_layer(0.0), None, "sem combate não há layer");

        combat.engage(10.0, false);
        assert_eq!(combat.active_layer(10.0), Some("battle"));
        assert_eq!(
            combat.active_layer(10.0 + COMBAT_MUSIC_HOLD as f64 - 0.5),
            Some("battle")
        );
        assert_eq!(
            combat.active_layer(10.0 + COMBAT_MUSIC_HOLD as f64 + 0.5),
            None,
            "expira sozinho após o hold"
        );

        // Refrescos prolongam; boss promove e não desce a battle.
        combat.engage(20.0, false);
        combat.engage(22.0, true);
        assert_eq!(combat.active_layer(27.0), Some("boss"), "boss > battle");
        // 22 + hold = 30; a 31 já ninguém renovou.
        assert_eq!(combat.active_layer(31.0), None);
    }

    #[test]
    fn test_combat_music_beats_zone_in_driver_target() {
        // layer_target recebe a zona resolvida — com combate ativo a layer
        // "battle" tem de ser a ativa mesmo em pé na vila.
        assert!(layer_target("battle", "battle", 0.24) > 0.1);
        assert_eq!(layer_target("village", "battle", 0.2), 0.0);
    }

    #[test]
    fn test_audio_duck_expires_and_clamps() {
        let mut duck = AudioDuck::default();
        assert_eq!(duck.ducked(AudioBus::Music, 0.0), 1.0);
        duck.apply(AudioBus::Music, 0.35, 2.0, 10.0);
        assert!((duck.ducked(AudioBus::Music, 10.5) - 0.35).abs() < 1e-6);
        assert_eq!(duck.ducked(AudioBus::Music, 13.0), 1.0, "expirou");
        // Ganhos fora de 0..=1 são presos; pedidos inválidos não mexem.
        duck.apply(AudioBus::Sfx, 9.0, 1.0, 0.0);
        assert_eq!(duck.ducked(AudioBus::Sfx, 0.5), 1.0);
        duck.apply(AudioBus::Sfx, 0.5, 0.0, 0.0);
        assert_eq!(duck.ducked(AudioBus::Sfx, 0.5), 1.0, "secs<=0 ignora");
    }

    #[test]
    fn test_overlapping_ducks_do_not_cancel_each_other() {
        let mut duck = AudioDuck::default();
        // Diálogo a 0.35 durante 8 s…
        duck.apply(AudioBus::Music, 0.35, 8.0, 100.0);
        // …e uma cinemática a 0.9 a meio: o diálogo continua a mandar.
        duck.apply(AudioBus::Music, 0.9, 30.0, 101.0);
        assert!((duck.ducked(AudioBus::Music, 102.0) - 0.35).abs() < 1e-6);
        // Quando o diálogo expira, fica só o duck de 0.9 (até aos 131 s).
        assert!((duck.ducked(AudioBus::Music, 110.0) - 0.9).abs() < 1e-6);
        assert!((duck.ducked(AudioBus::Music, 120.0) - 0.9).abs() < 1e-6);
        assert_eq!(duck.ducked(AudioBus::Music, 132.0), 1.0, "ambos expiraram");
    }

    #[test]
    fn test_reverb_presets_are_ordered_and_parsed() {
        assert_eq!(ReverbPreset::parse("off"), Some(ReverbPreset::Off));
        assert_eq!(ReverbPreset::parse("room"), Some(ReverbPreset::Room));
        assert_eq!(ReverbPreset::parse("cave"), Some(ReverbPreset::Cave));
        assert_eq!(ReverbPreset::parse("banheiro"), None);
        let (_, _, off) = ReverbPreset::Off.params();
        let (_, _, room) = ReverbPreset::Room.params();
        let (_, _, cave) = ReverbPreset::Cave.params();
        assert_eq!(off, 0.0);
        assert!(room > off && cave > room, "mais molhado por preset");
    }
}
