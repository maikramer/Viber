//! A VM Luau partilhada ([`LuaScriptHost`]): carregamento e cache de chunks
//! por path, activação do top-level, chamada dos hooks (`on_update`,
//! `on_player_attack`) e o livro de warn-once. Cada chunk corre num
//! environment sandbox próprio (`__index` → globals reais).

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use bevy::prelude::*;
use mlua::{Function, Lua, LuaOptions, StdLib, Table, VmState};

use super::commands::ScriptCommand;
use super::ctx::ScriptCtx;

/// A compiled script chunk with its sandboxed environment.
#[derive(Clone)]
pub struct LoadedScript {
    /// Per-script global environment (`__index` falls back to real globals,
    /// so the shared `viber` API table stays visible).
    pub env: Table,
    /// Top-level chunk (executed once, defines `on_update` in `env`).
    pub chunk: Function,
    /// `on_update(dt)` extracted from the environment after execution.
    pub on_update: Option<Function>,
    /// Callback opcional de aggro-chain (`on_player_attack(px, pz)`).
    pub on_player_attack: Option<Function>,
    /// `on_trigger_enter(other)` — sensor Rapier.
    pub on_trigger_enter: Option<Function>,
    /// `on_trigger_stay(other, dt)`.
    pub on_trigger_stay: Option<Function>,
    /// `on_trigger_exit(other)`.
    pub on_trigger_exit: Option<Function>,
    /// `on_collision_enter(other)` — collider sólido.
    pub on_collision_enter: Option<Function>,
    /// `on_collision_stay(other, dt)`.
    pub on_collision_stay: Option<Function>,
    /// `on_collision_exit(other)`.
    pub on_collision_exit: Option<Function>,
    /// True once the chunk's top-level has run.
    pub ran: bool,
}

/// Qual hook de contacto chamar. Stay leva `(other, dt)`; os outros só `other`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContactHook {
    /// Sensor começou a intersectar.
    TriggerEnter,
    /// Sensor continua a intersectar.
    TriggerStay,
    /// Sensor deixou de intersectar.
    TriggerExit,
    /// Collider sólido começou o contacto.
    CollisionEnter,
    /// Collider sólido continua em contacto.
    CollisionStay,
    /// Collider sólido deixou o contacto.
    CollisionExit,
}

impl ContactHook {
    /// Stay passa o `dt` além da tabela `other`.
    pub fn is_stay(self) -> bool {
        matches!(self, Self::TriggerStay | Self::CollisionStay)
    }

    pub(crate) fn enter(trigger: bool) -> Self {
        if trigger {
            Self::TriggerEnter
        } else {
            Self::CollisionEnter
        }
    }

    pub(crate) fn exit(trigger: bool) -> Self {
        if trigger {
            Self::TriggerExit
        } else {
            Self::CollisionExit
        }
    }

    pub(crate) fn stay(trigger: bool) -> Self {
        if trigger {
            Self::TriggerStay
        } else {
            Self::CollisionStay
        }
    }
}

/// Registry of loaded chunks: `HashMap<path → loaded chunk handle>`.
#[derive(Default)]
pub struct LuaScriptRegistry {
    chunks: HashMap<String, LoadedScript>,
}

impl LuaScriptRegistry {
    pub fn get(&self, path: &str) -> Option<&LoadedScript> {
        self.chunks.get(path)
    }

    pub fn get_mut(&mut self, path: &str) -> Option<&mut LoadedScript> {
        self.chunks.get_mut(path)
    }

    pub fn contains(&self, path: &str) -> bool {
        self.chunks.contains_key(path)
    }

    pub fn insert(&mut self, path: String, script: LoadedScript) {
        self.chunks.insert(path, script);
    }

    /// Paths of all loaded scripts (sorted, for stable logging).
    pub fn paths(&self) -> Vec<&str> {
        let mut paths: Vec<&str> = self.chunks.keys().map(String::as_str).collect();
        paths.sort_unstable();
        paths
    }
}

/// Tempo máximo de UMA entrada na VM (hook, top-level, timer, `on_spawned`)
/// antes de a interromper com erro — um `while true do end` num script
/// congelava a engine inteira.
pub const SCRIPT_CALL_BUDGET: Duration = Duration::from_millis(500);

/// Teto de heap da VM partilhada: um script a crescer uma tabela sem fim
/// falha com erro Lua em vez de levar o processo ao OOM.
pub const SCRIPT_MEMORY_LIMIT: usize = 512 * 1024 * 1024;

/// Relógio do orçamento: prazo em ns desde `epoch` (0 = sem prazo). O
/// interrupt do Luau lê-o; o relógio só é consultado a cada 64 interrupts.
struct BudgetClock {
    epoch: Instant,
    deadline_ns: AtomicU64,
    ticks: AtomicU32,
}

/// Arma o orçamento enquanto vive; ao sair repõe o prazo anterior.
pub struct BudgetGuard {
    clock: Arc<BudgetClock>,
    previous: u64,
}

impl Drop for BudgetGuard {
    fn drop(&mut self) {
        self.clock
            .deadline_ns
            .store(self.previous, Ordering::Relaxed);
    }
}

/// Bevy resource holding the shared Luau VM, the chunk registry and the
/// warn-once bookkeeping. Build with [`LuaScriptHost::new`], load scripts via
/// [`LuaScriptHost::load_script`] (code string) or
/// [`LuaScriptHost::load_script_from_dir`] (disk), then add
/// [`LuauScriptPlugin`]-equivalent systems.
#[derive(Resource)]
pub struct LuaScriptHost {
    /// The shared sandboxed Luau VM (`Lua::new()` with the `luau` feature).
    pub lua: Lua,
    /// Loaded chunks keyed by script path.
    pub registry: LuaScriptRegistry,
    /// Directory scripts are loaded from: `<world_dir>/scripts`.
    pub scripts_dir: PathBuf,
    /// Script paths whose last error was already warned (warn 1x).
    warned: HashSet<String>,
    budget: Arc<BudgetClock>,
}

impl LuaScriptHost {
    /// Creates the VM, installs the `viber` API table and seeds the app-data
    /// [`ScriptCtx`].
    pub fn new(scripts_dir: PathBuf) -> mlua::Result<Self> {
        // ALL_SAFE no Luau = TODAS as libs do Luau (mlua 0.12): o Luau não
        // tem `package`/`os`/`io` — o `require` de módulos passa pelo
        // `viber.load`, preso a `scripts/`.
        let lua = Lua::new_with(StdLib::ALL_SAFE, LuaOptions::default())?;
        lua.set_memory_limit(SCRIPT_MEMORY_LIMIT)?;
        // Luau JIT (feature `luau-jit`): chunks novos compilam para código
        // nativo. A rede de segurança é o `set_interrupt` lá em baixo —
        // `test_infinite_loop_is_interrupted_by_the_cpu_budget` prova que o
        // orçamento interrompe TAMBÉM em código JIT. `VIBER_LUAU_JIT=0`
        // volta ao interpretador puro (A/B de perf no mesmo binário).
        #[cfg(feature = "luau-jit")]
        {
            let jit = std::env::var("VIBER_LUAU_JIT")
                .map(|v| v != "0")
                .unwrap_or(true);
            lua.enable_jit(jit);
        }
        // mlua 0.12 trouxe o require-by-string do Luau: o global nasce na VM
        // e leria ficheiros do disco — módulos passam pelo `viber.load`.
        lua.globals().set("require", mlua::Value::Nil)?;
        // Biblioteca padrão só-leitura: os envs por script isolam globals,
        // mas `string.format = nil` num script mexia na tabela PARTILHADA.
        for lib in [
            "string",
            "math",
            "table",
            "coroutine",
            "bit32",
            "utf8",
            "os",
            "buffer",
        ] {
            if let Ok(t) = lua.globals().get::<Table>(lib) {
                t.set_readonly(true);
            }
        }
        let budget = Arc::new(BudgetClock {
            epoch: Instant::now(),
            deadline_ns: AtomicU64::new(0),
            ticks: AtomicU32::new(0),
        });
        let clock = budget.clone();
        lua.set_interrupt(move |_| {
            let deadline = clock.deadline_ns.load(Ordering::Relaxed);
            if deadline == 0
                || !clock
                    .ticks
                    .fetch_add(1, Ordering::Relaxed)
                    .is_multiple_of(64)
            {
                return Ok(VmState::Continue);
            }
            if clock.epoch.elapsed().as_nanos() as u64 >= deadline {
                return Err(mlua::Error::runtime(format!(
                    "orçamento de CPU excedido ({} ms numa chamada) — loop infinito?",
                    SCRIPT_CALL_BUDGET.as_millis()
                )));
            }
            Ok(VmState::Continue)
        });
        let host = Self {
            lua,
            registry: LuaScriptRegistry::default(),
            scripts_dir,
            warned: HashSet::new(),
            budget,
        };
        host.install_viber_api()?;
        host.lua.set_app_data(ScriptCtx::default());
        {
            let mut ctx = host
                .lua
                .app_data_mut::<ScriptCtx>()
                .expect("ScriptCtx app data seeded in LuaScriptHost::new");
            ctx.scripts_dir = Some(host.scripts_dir.clone());
        }
        Ok(host)
    }

    /// Arma o [`SCRIPT_CALL_BUDGET`] até o guard sair de escopo. Entradas na
    /// VM sem guard (REPL do bridge, testes) correm sem prazo.
    pub fn budget_guard(&self) -> BudgetGuard {
        let deadline = self.budget.epoch.elapsed() + SCRIPT_CALL_BUDGET;
        let previous = self
            .budget
            .deadline_ns
            .swap((deadline.as_nanos() as u64).max(1), Ordering::Relaxed);
        BudgetGuard {
            clock: self.budget.clone(),
            previous,
        }
    }

    /// Compiles `code` under `path` with a fresh sandboxed environment.
    /// The chunk is *not* executed yet — [`LuaScriptHost::activate`] runs the
    /// top level when the first entity references the script.
    pub fn load_script(&mut self, path: &str, code: &str) -> mlua::Result<()> {
        let env = self.create_script_env()?;
        let chunk = self
            .lua
            .load(code)
            .set_name(path)
            .set_environment(env.clone())
            .into_function()?;
        let exports = super::exports::parse_exports(code);
        for error in &exports.errors {
            warn!("[luau] {path}: --@export {error}");
        }
        if let Some(mut ctx) = self.lua.app_data_mut::<ScriptCtx>() {
            ctx.exports.insert(path.to_string(), Arc::new(exports));
        }
        self.registry.insert(
            path.to_string(),
            LoadedScript {
                env,
                chunk,
                on_update: None,
                on_player_attack: None,
                on_trigger_enter: None,
                on_trigger_stay: None,
                on_trigger_exit: None,
                on_collision_enter: None,
                on_collision_stay: None,
                on_collision_exit: None,
                ran: false,
            },
        );
        // NOTA: os timers do chunk antigo NÃO morrem aqui. No reload o
        // `crate::hot_reload::reload_script` deita-os só DEPOIS de o novo
        // top-level correr bem (num erro o chunk antigo é reposto COM os
        // seus timers). Um drop aqui matava-os nesse caminho de falha.
        Ok(())
    }

    /// Loads a script from disk: `<scripts_dir>/<path>`.
    pub fn load_script_from_dir(&mut self, path: &str) -> mlua::Result<()> {
        // O lint aceita o estilo antigo `scripts/x.lua`; o runtime tem de
        // resolver o mesmo caminho (a chave do registry fica a autoral).
        let relative = path.strip_prefix("scripts/").unwrap_or(path);
        // Mesma regra do `viber.load`/`load_json`: um `script="../../x.lua"`
        // (ou um caminho absoluto) lia e executava um ficheiro FORA de
        // `scripts/` — e a lint só via o caminho autoral.
        let relative_path = std::path::Path::new(relative);
        let inside = !relative.is_empty()
            && relative_path.components().all(|component| {
                matches!(
                    component,
                    std::path::Component::Normal(_) | std::path::Component::CurDir
                )
            });
        if !inside {
            return Err(mlua::Error::runtime(format!(
                "script '{path}': só caminhos relativos a scripts/ (sem '..')"
            )));
        }
        let full = self.scripts_dir.join(relative_path);
        let code = std::fs::read_to_string(&full).map_err(|e| {
            mlua::Error::runtime(format!("failed to read script {}: {e}", full.display()))
        })?;
        self.load_script(path, &code)
    }

    /// Ensures `path` is in the registry, loading it from
    /// `<scripts_dir>/<path>` when missing.
    pub fn ensure_loaded(&mut self, path: &str) -> mlua::Result<()> {
        if self.registry.contains(path) {
            return Ok(());
        }
        self.load_script_from_dir(path)
    }

    /// Runs the chunk top level once (defines `on_update` and any script
    /// state) and extracts the `on_update` handle. Idempotent per path —
    /// entities sharing a script share its globals.
    pub fn activate(&mut self, entity: Entity, path: &str) -> mlua::Result<()> {
        self.activate_at(entity, path, Vec3::ZERO)
    }

    /// [`LuaScriptHost::activate`] com a posição de spawn real — o `home`
    /// (centro do wander) é gravado a partir dela. `ctx.origin` chega aqui
    /// stale (só `run_update` o actualiza, e corre DEPOIS de `on_add` no
    /// primeiro frame), por isso a posição tem de vir de fora.
    pub fn activate_at(&mut self, entity: Entity, path: &str, origin: Vec3) -> mlua::Result<()> {
        // Refresh the per-call ctx so top-level viber calls don't panic.
        let origin = if let Some(mut ctx) = self.lua.app_data_mut::<ScriptCtx>() {
            ctx.entity = Some(entity);
            ctx.set_path(path);
            ctx.origin = origin;
            // Nome/props são POR ENTIDADE e só existem depois de um hook ou
            // do snapshot do frame; o top-level de uma instância nova lia o
            // nome/props da ÚLTIMA entidade que correu (o mesmo bug que os
            // hooks já não têm). Sem dados, nil — nunca o vizinho.
            ctx.self_name = None;
            ctx.self_props = None;
            // O mesmo para o alcance de interação: sem reset, o top-level
            // via o range da última entidade do `on_update` anterior.
            ctx.interaction_range = None;
            ctx.origin
        } else {
            Vec3::ZERO
        };
        // Home = posição de spawn (centro do wander), gravada uma vez.
        let key = entity.to_bits() as i64;
        {
            let states: Table = self.lua.named_registry_value("viber_states")?;
            let table = states.raw_get::<Table>(key).or_else(|_| {
                let fresh = self.lua.create_table()?;
                let _ = states.raw_set(key, fresh.clone());
                Ok::<Table, mlua::Error>(fresh)
            })?;
            if table.raw_get::<Table>("home").is_err() {
                let home = self.lua.create_table()?;
                home.raw_set("x", origin.x)?;
                home.raw_set("z", origin.z)?;
                table.raw_set("home", home)?;
                table.raw_set("picks", 0u64)?;
            }
        }
        let script = self
            .registry
            .get_mut(path)
            .ok_or_else(|| mlua::Error::runtime(format!("script '{path}' not loaded")))?;
        if !script.ran {
            let chunk = script.chunk.clone();
            let _budget = self.budget_guard();
            if let Some(mut ctx) = self.lua.app_data_mut::<ScriptCtx>() {
                ctx.top_level = true;
            }
            let ran = chunk.call::<()>(());
            if let Some(mut ctx) = self.lua.app_data_mut::<ScriptCtx>() {
                ctx.top_level = false;
            }
            let script = self
                .registry
                .get_mut(path)
                .ok_or_else(|| mlua::Error::runtime(format!("script '{path}' not loaded")))?;
            // Marca o chunk como executado ANTES de extrair os hooks: um
            // global de hook com o tipo errado (ex. `on_update = true`) faz
            // `raw_get` falhar; com `ran` ainda falso, cada `on_add`/contacto
            // re-corria o TOP-LEVEL (timers e efeitos do topo repetiam-se a
            // cada toque). O erro continua a ser reportado uma vez.
            //
            // Também marca antes de propagar um erro do PRÓPRIO top-level:
            // o top-level corre UMA vez por path (contrato), e um script que
            // rebenta depois de registar `viber.every`/`viber.after`
            // re-registava o timer por cada entidade nova que o usasse. Os
            // hooks definidos antes do erro ficam extraídos (o parcial
            // continua a correr; os erros são dados, não fatais).
            script.ran = true;
            script.on_update = script.env.raw_get::<Option<Function>>("on_update")?;
            script.on_player_attack = script.env.raw_get::<Option<Function>>("on_player_attack")?;
            script.on_trigger_enter = script.env.raw_get::<Option<Function>>("on_trigger_enter")?;
            script.on_trigger_stay = script.env.raw_get::<Option<Function>>("on_trigger_stay")?;
            script.on_trigger_exit = script.env.raw_get::<Option<Function>>("on_trigger_exit")?;
            script.on_collision_enter = script
                .env
                .raw_get::<Option<Function>>("on_collision_enter")?;
            script.on_collision_stay = script
                .env
                .raw_get::<Option<Function>>("on_collision_stay")?;
            script.on_collision_exit = script
                .env
                .raw_get::<Option<Function>>("on_collision_exit")?;
            // O erro do top-level continua a chegar ao chamador (warn 1× via
            // `warn_once`) — mas o chunk já está marcado como executado.
            ran?;
        }
        Ok(())
    }

    /// Calls `on_update(dt)` for `entity`'s script. The [`ScriptCtx`] snapshot
    /// (entity, origin, player, clock) must be passed in; commands queued by
    /// `viber.set_position` accumulate until [`LuaScriptHost::take_pending`].
    /// Scripts without `on_update` are a no-op.
    #[allow(clippy::too_many_arguments)]
    pub fn run_update(
        &mut self,
        entity: Entity,
        path: &str,
        dt: f32,
        origin: Vec3,
        player: Option<Vec3>,
        elapsed: f64,
    ) -> mlua::Result<()> {
        {
            let mut ctx = self
                .lua
                .app_data_mut::<ScriptCtx>()
                .expect("ScriptCtx app data seeded in LuaScriptHost::new");
            ctx.entity = Some(entity);
            ctx.set_path(path);
            ctx.origin = origin;
            ctx.player = player;
            ctx.dt = dt;
            ctx.elapsed = elapsed;
        } // release the borrow before re-entering Lua
        let on_update = self.registry.get(path).and_then(|s| s.on_update.clone());
        let Some(on_update) = on_update else {
            return Ok(());
        };
        let _budget = self.budget_guard();
        on_update.call::<()>(dt)
    }

    /// Aggro-chain: chama `on_player_attack(px, pz)` no script da entidade
    /// (opcional — scripts sem o callback são ignorados).
    pub fn run_player_attack_alert(
        &mut self,
        entity: Entity,
        path: &str,
        origin: Vec3,
        attacker_pos: Vec3,
        self_name: Option<String>,
        self_props: Option<std::sync::Arc<Vec<(String, String)>>>,
    ) -> mlua::Result<()> {
        {
            let mut ctx = self
                .lua
                .app_data_mut::<ScriptCtx>()
                .expect("ScriptCtx app data seeded in LuaScriptHost::new");
            ctx.entity = Some(entity);
            ctx.set_path(path);
            // Sem isto, on_player_attack corria com origin/dt/elapsed da
            // ÚLTIMA entidade do frame — position()/move_towards/damage_player
            // calculavam a partir de outra criatura.
            ctx.origin = origin;
            ctx.self_name = self_name;
            ctx.self_props = self_props;
            // O hook não tem range autorado próprio (`set_interaction` só
            // aplica pós-frame): sem isto a callback lia o range de OUTRA
            // entidade, semeado pelo último `on_update` do frame anterior.
            ctx.interaction_range = None;
        }
        let cb = self
            .registry
            .get(path)
            .and_then(|s| s.on_player_attack.clone());
        let Some(cb) = cb else {
            return Ok(());
        };
        let _budget = self.budget_guard();
        cb.call::<()>((attacker_pos.x, attacker_pos.z))
    }

    /// Hook de collider/trigger. `stay` passa `(other, dt)`; enter/exit só
    /// `other` (`{id, name?, x, y, z}`). `Ok(())` quando o script não define
    /// o hook — o chamador não avisa.
    #[allow(clippy::too_many_arguments)]
    pub fn run_contact(
        &mut self,
        entity: Entity,
        path: &str,
        origin: Vec3,
        hook: ContactHook,
        other_id: i64,
        other_name: Option<&str>,
        other_pos: Vec3,
        dt: f32,
        self_name: Option<String>,
        self_props: Option<std::sync::Arc<Vec<(String, String)>>>,
    ) -> mlua::Result<()> {
        {
            let mut ctx = self
                .lua
                .app_data_mut::<ScriptCtx>()
                .expect("ScriptCtx app data seeded in LuaScriptHost::new");
            ctx.entity = Some(entity);
            ctx.set_path(path);
            ctx.origin = origin;
            ctx.dt = dt;
            // Um hook de `tree.lua` partilhado lia o `self_name`/props do
            // último `on_update` (outra árvore) — `viber.prop` mentia. O
            // `interaction_range` também é por entidade: um contacto não tem
            // range autorado (o `set_interaction` aplica pós-frame).
            ctx.self_name = self_name;
            ctx.self_props = self_props;
            ctx.interaction_range = None;
        }
        let callback = self.registry.get(path).and_then(|script| match hook {
            ContactHook::TriggerEnter => script.on_trigger_enter.clone(),
            ContactHook::TriggerStay => script.on_trigger_stay.clone(),
            ContactHook::TriggerExit => script.on_trigger_exit.clone(),
            ContactHook::CollisionEnter => script.on_collision_enter.clone(),
            ContactHook::CollisionStay => script.on_collision_stay.clone(),
            ContactHook::CollisionExit => script.on_collision_exit.clone(),
        });
        let Some(callback) = callback else {
            return Ok(());
        };
        let other = self.lua.create_table()?;
        other.raw_set("id", other_id)?;
        other.raw_set("x", other_pos.x)?;
        other.raw_set("y", other_pos.y)?;
        other.raw_set("z", other_pos.z)?;
        if let Some(name) = other_name {
            other.raw_set("name", name)?;
        }
        let _budget = self.budget_guard();
        if hook.is_stay() {
            callback.call::<()>((other, dt))
        } else {
            callback.call::<()>(other)
        }
    }

    /// Entrega um sinal/`call_group`: chama a função global `method` do script
    /// de `entity` com `args` e `viber.sender()` = `sender`. `Ok(false)` quando
    /// o script não define o método (o chamador avisa).
    pub fn run_method(
        &mut self,
        entity: Entity,
        path: &str,
        origin: Vec3,
        method: &str,
        args: mlua::MultiValue,
        sender: Option<i64>,
    ) -> mlua::Result<bool> {
        let Some(script) = self.registry.get(path) else {
            return Ok(false);
        };
        let Some(func) = script.env.raw_get::<Option<Function>>(method)? else {
            return Ok(false);
        };
        {
            let mut ctx = self
                .lua
                .app_data_mut::<ScriptCtx>()
                .expect("ScriptCtx app data seeded in LuaScriptHost::new");
            ctx.entity = Some(entity);
            ctx.set_path(path);
            ctx.origin = origin;
            ctx.sender = sender;
        }
        let result = {
            let _budget = self.budget_guard();
            func.call::<()>(args)
        };
        if let Some(mut ctx) = self.lua.app_data_mut::<ScriptCtx>() {
            ctx.sender = None;
        }
        result.map(|()| true)
    }

    /// Drains all queued `set_position` commands (called once per frame after
    /// every script ran).
    pub fn take_pending(&self) -> Vec<(Entity, Vec3)> {
        self.lua
            .app_data_mut::<ScriptCtx>()
            .map(|mut ctx| std::mem::take(&mut ctx.pending))
            .unwrap_or_default()
    }

    /// `viber.log` lines so far (ring buffer, oldest first).
    pub fn logs(&self) -> Vec<String> {
        self.lua
            .app_data_ref::<ScriptCtx>()
            .map(|ctx| ctx.logs.clone())
            .unwrap_or_default()
    }

    /// Reads a global value from a script's sandboxed environment (test/HUD
    /// introspection helper).
    pub fn script_global(&self, path: &str, key: &str) -> mlua::Result<mlua::Value> {
        let script = self
            .registry
            .get(path)
            .ok_or_else(|| mlua::Error::runtime(format!("script '{path}' not loaded")))?;
        script.env.raw_get(key)
    }

    /// Reports `err` for `path`; returns true the first time (so callers only
    /// emit a `warn!` once per script). Never panics — script errors are
    /// data, engine keeps running.
    pub fn warn_once(&mut self, path: &str, err: &dyn std::fmt::Display) -> bool {
        if self.warned.insert(path.to_string()) {
            warn!("luau script '{path}' error (further errors silenced): {err}");
            true
        } else {
            false
        }
    }

    /// Clears warn-once state for `path` (e.g. after a successful reload).
    pub fn clear_warnings(&mut self, path: &str) {
        self.warned.remove(path);
    }

    /// Removes per-entity leftovers when a [`LuaScriptRef`] despawns (drops
    /// queued commands for that entity; the chunk stays cached for reuse).
    pub fn deactivate(&mut self, entity: Entity) {
        // Estado por entidade: limpa a tabela Lua (respawn = estado fresco).
        if let Ok(states) = self.lua.named_registry_value::<Table>("viber_states") {
            let _ = states.raw_remove(entity.to_bits() as i64);
        }
        // Timers do dono morrem com ele (um respawn re-regista os seus).
        let bits = entity.to_bits() as i64;
        let orphans = super::timers::timer_ids_where(&self.lua, |owner, _| owner == bits);
        super::timers::drop_timers(&self.lua, &orphans);
        if let Some(mut ctx) = self.lua.app_data_mut::<ScriptCtx>() {
            ctx.pending.retain(|(e, _)| *e != entity);
            ctx.commands.retain(|c| {
                // Só os comandos bound à entidade morrem com ela (quests/
                // toasts/vault são world-scoped); os restantes ficam.
                let owner = match c {
                    ScriptCommand::MoveBy(e, _)
                    | ScriptCommand::FaceTowards(e, _)
                    | ScriptCommand::SetInteraction { entity: e, .. }
                    | ScriptCommand::Despawn(e)
                    | ScriptCommand::Gesture { entity: e, .. }
                    | ScriptCommand::SetPose { entity: e, .. } => *e,
                    _ => return true,
                };
                owner != entity
            });
            if ctx.entity == Some(entity) {
                ctx.entity = None;
            }
        }
    }

    /// Fresh per-script environment whose metatable falls back to the real
    /// globals (so the shared `viber` API and stdlib stay reachable without
    /// letting scripts overwrite each other's globals).
    fn create_script_env(&self) -> mlua::Result<Table> {
        let env = self.lua.create_table()?;
        let mt = self.lua.create_table()?;
        mt.set("__index", self.lua.globals())?;
        // mlua 0.12: devolve Result (pode falhar em tabelas readonly).
        env.set_metatable(Some(mt))?;
        Ok(env)
    }

    /// Instala a API `viber` na VM — composta em `api.rs` (grupos
    /// categorizados: núcleo, input, eventos, timers, entidade, jogo).
    fn install_viber_api(&self) -> mlua::Result<()> {
        super::api::install(&self.lua)
    }

    /// JSON do `viber.state()` desta entidade (números, strings, bools e
    /// tabelas de um nível). `None` se ainda não há estado.
    pub fn dump_entity_state(&self, entity: Entity) -> Option<String> {
        let states: Table = self.lua.named_registry_value("viber_states").ok()?;
        let table: Table = states.raw_get(entity.to_bits() as i64).ok()?;
        let value = lua_table_to_json(&table);
        serde_json::to_string(&value).ok()
    }

    /// Repõe o `viber.state()` gravado por [`Self::dump_entity_state`] na
    /// entidade nova (o id muda quando o streaming a volta a spawnar).
    pub fn restore_entity_state(&self, entity: Entity, json: &str) -> mlua::Result<()> {
        let value: serde_json::Value =
            serde_json::from_str(json).map_err(|e| mlua::Error::runtime(e.to_string()))?;
        let states: Table = self.lua.named_registry_value("viber_states")?;
        let table = json_to_lua_table(&self.lua, &value)?;
        states.raw_set(entity.to_bits() as i64, table)?;
        Ok(())
    }
}

/// Profundidade máxima de uma tabela de estado. O `viber.state()` é definido
/// pelo script e pode conter ciclos (`st.self = st`): sem teto, o dump do
/// streaming recursava até estourar a pilha e abortava o processo.
const STATE_JSON_MAX_DEPTH: usize = 32;

fn lua_table_to_json(table: &Table) -> serde_json::Value {
    lua_table_to_json_at(table, 0)
}

fn lua_table_to_json_at(table: &Table, depth: usize) -> serde_json::Value {
    if depth >= STATE_JSON_MAX_DEPTH {
        return serde_json::Value::Null;
    }
    let mut map = serde_json::Map::new();
    let mut numeric: Vec<(i64, mlua::Value)> = Vec::new();
    let mut array_shaped = true;
    for pair in table.pairs::<mlua::Value, mlua::Value>() {
        let Ok((key, value)) = pair else { continue };
        match key {
            mlua::Value::Integer(n) if n >= 1 => numeric.push((n, value)),
            mlua::Value::Integer(n) => {
                array_shaped = false;
                if let Some(json) = lua_value_to_json_at(value, depth + 1) {
                    map.insert(n.to_string(), json);
                }
            }
            mlua::Value::String(text) => {
                array_shaped = false;
                if let Some(json) = lua_value_to_json_at(value, depth + 1) {
                    map.insert(text.to_string_lossy(), json);
                }
            }
            _ => array_shaped = false,
        }
    }
    // Sequência 1..n contígua vira array JSON; sem isto `st.queue[1]`
    // voltava como a chave string "1" e lia-se `nil` depois do streaming.
    if array_shaped && !numeric.is_empty() {
        numeric.sort_by_key(|(index, _)| *index);
        if numeric
            .iter()
            .enumerate()
            .all(|(position, (index, _))| *index == position as i64 + 1)
        {
            return serde_json::Value::Array(
                numeric
                    .into_iter()
                    .filter_map(|(_, value)| lua_value_to_json_at(value, depth + 1))
                    .collect(),
            );
        }
    }
    for (index, value) in numeric {
        if let Some(json) = lua_value_to_json_at(value, depth + 1) {
            map.insert(index.to_string(), json);
        }
    }
    serde_json::Value::Object(map)
}

fn lua_value_to_json_at(value: mlua::Value, depth: usize) -> Option<serde_json::Value> {
    match value {
        mlua::Value::Nil => Some(serde_json::Value::Null),
        mlua::Value::Boolean(v) => Some(serde_json::Value::Bool(v)),
        mlua::Value::Integer(v) => Some(serde_json::Value::from(v)),
        mlua::Value::Number(v) => serde_json::Number::from_f64(v).map(serde_json::Value::Number),
        mlua::Value::String(text) => Some(serde_json::Value::String(text.to_string_lossy())),
        mlua::Value::Table(table) => Some(lua_table_to_json_at(&table, depth)),
        _ => None,
    }
}

fn json_to_lua_table(lua: &mlua::Lua, value: &serde_json::Value) -> mlua::Result<Table> {
    match json_to_lua_value(lua, value)? {
        mlua::Value::Table(table) => Ok(table),
        _ => lua.create_table(),
    }
}

fn json_to_lua_value(lua: &mlua::Lua, value: &serde_json::Value) -> mlua::Result<mlua::Value> {
    Ok(match value {
        serde_json::Value::Null => mlua::Value::Nil,
        serde_json::Value::Bool(v) => mlua::Value::Boolean(*v),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                mlua::Value::Integer(i)
            } else if let Some(f) = n.as_f64() {
                mlua::Value::Number(f)
            } else {
                mlua::Value::Nil
            }
        }
        serde_json::Value::String(text) => mlua::Value::String(lua.create_string(text)?),
        serde_json::Value::Array(items) => {
            let table = lua.create_table()?;
            for (index, item) in items.iter().enumerate() {
                table.raw_set(index as i64 + 1, json_to_lua_value(lua, item)?)?;
            }
            mlua::Value::Table(table)
        }
        serde_json::Value::Object(map) => {
            let table = lua.create_table()?;
            for (key, value) in map {
                // Chaves de tabela NUMÉRICAS: o dump (`lua_table_to_json`)
                // grava-as como texto porque JSON só tem chaves string. Sem
                // as converter de volta, `st.inside[bits]` (o padrão de um
                // script que indexa por id) lia nil depois do streaming
                // dormir/acordar a entidade. Só a forma canónica (`7`, `0`,
                // `-3`) volta a inteiro: "07"/"1.5" ficam strings.
                match key.parse::<i64>() {
                    Ok(index) if index.to_string() == key.as_str() => {
                        table.raw_set(index, json_to_lua_value(lua, value)?)?;
                    }
                    _ => table.raw_set(key.as_str(), json_to_lua_value(lua, value)?)?,
                }
            }
            mlua::Value::Table(table)
        }
    })
}
