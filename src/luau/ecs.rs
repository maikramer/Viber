//! `viber.ecs`: acesso ECS genérico a componentes registados por reflexão.
//!
//! É a ponte fina entre o [`crate::reflect::ReflectPlugin`] e o Luau: em vez
//! de um `ScriptCommand` por conceito de jogo, os scripts passam a ler e
//! escrever componentes pelo `TypePath` ([`WHITELIST`] curada — nunca um path
//! arbitrário de Rust).
//!
//! Desenho (o mesmo do resto da API `viber.*`):
//! - **Leituras síncronas** (`get`/`query`/`types`) vêm de um espelho
//!   ([`EcsMirrorData`]) reconstruído no início do frame por
//!   [`mirror_system`] — as closures Lua não tocam no `World` (sem `unsafe`,
//!   sem ponteiros crus). Como todo o snapshot da API, o valor é do início do
//!   frame: uma escrita feita por um script aparece no frame seguinte.
//! - **Escritas** (`set`/`insert`/`remove`/`spawn`/`despawn`) enfileiram um
//!   [`EcsOp`] em [`ScriptCommand::Ecs`], aplicado pós-frame pelo
//!   `luau_update` com `Commands::queue` (ordem e ciclo de vida do resto dos
//!   comandos).
//! - `spawn` devolve já um id válido: [`EcsSpawnPool`] mantém entidades vazias
//!   pré-criadas por [`mirror_system`] e a closure Lua limita-se a tirar uma.
//!
//! O espelho só converte os componentes da whitelist e só corre com scripts
//! vivos (`LuaScriptRef`), para um mundo estático (editor, menu) não pagar
//! nada.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use bevy::ecs::reflect::ReflectComponent;
use bevy::prelude::*;
use bevy::reflect::{
    PartialReflect, ReflectFromReflect, ReflectMut, ReflectRef, TypeInfo, TypeRegistration,
    TypeRegistry,
    enums::{DynamicEnum, DynamicVariant, Enum, EnumInfo, VariantType},
    std_traits::ReflectDefault,
    structs::DynamicStruct,
    tuple::DynamicTuple,
    tuple_struct::DynamicTupleStruct,
};
use mlua::{IntoLua, Lua, Table, Value};

use super::commands::ScriptCommand;
use super::ctx::ScriptCtx;

/// Tipos que o Luau pode ver/escrever por `viber.ecs` (`viber.ecs.types()`).
///
/// Curada: só dados de JOGO que um script tem motivo para inspecionar. Ficam
/// de fora caches (GPU, Rapier, VM), tipos enormes (`CharacterAnimator`) e os
/// componentes de transformação/visibilidade — esses têm verbos próprios
/// (`viber.position`, `viber.set_pose`, `viber.set_visible`) e o espelho de
/// todos os `Transform` de um mundo grande (≈79k entidades no Vael) seria
/// desperdício por frame.
pub const WHITELIST: &[&str] = &[
    // Vitais e combate
    "viber::vitals::Health",
    "viber::vitals::Hostile",
    "viber::projectile::Projectile",
    "viber::spawn_gate::SpawnGated",
    // Jogador e IA
    "viber::player::Player",
    "viber::player::TeleportSettle",
    "viber::ai::AiLocomotion",
    "viber::ai::EnemyCreature",
    "viber::ai::WanderState",
    // Ligação entidade↔script
    "viber::luau::components::LuaScriptRef",
    "viber::luau::components::ScriptProps",
    "viber::luau::components::Groups",
    "viber::luau::components::SignalConnections",
    "viber::luau::components::ScriptActivation",
    "viber::luau::components::ScriptInteraction",
    // Câmara
    "viber::camera::BaseFov",
];

/// Caminho do `Transform` (escritas internas de `set_pose`/`set_scale`; NÃO
/// entra no espelho Lua).
pub const TRANSFORM_PATH: &str = "bevy_transform::components::transform::Transform";

/// Caminho da `Visibility` (escritas internas de `set_visible`).
pub const VISIBILITY_PATH: &str = "bevy_camera::visibility::Visibility";

/// Entidades vazias reservadas por frame para `viber.ecs.spawn` (mínimo).
pub const MIN_SPAWN_POOL: usize = 8;

/// Valor refletido convertido para uma forma independente da VM.
///
/// `Map` = struct com campos nomeados; `Seq` = tuplo/lista/array. Os nomes
/// seguem a API existente (`x`/`y`/`z`, `r`/`g`/`b`/`a`) para uma tabela Lua
/// poder voltar a entrar em `set` sem tradução.
#[derive(Debug, Clone, PartialEq)]
pub enum EcsValue {
    /// Struct/componente unitário (`Hostile`) ou variante sem dados.
    Unit,
    /// Ausência (`Option::None`, campo não representável). Em Lua vira `nil`.
    Nil,
    Bool(bool),
    Int(i64),
    UInt(u64),
    Float(f64),
    Str(String),
    Vec2([f32; 2]),
    Vec3([f32; 3]),
    Quat([f32; 4]),
    Color([f32; 4]),
    /// Bits da entidade (i64, como `viber.self_id()`).
    Entity(i64),
    /// Variante de enum sem payload (`Faction::Enemy` → `"Enemy"`).
    Enum(String),
    Map(Vec<(String, EcsValue)>),
    Seq(Vec<EcsValue>),
}

/// Uma escrita pedida por `viber.ecs.*`, aplicada pós-frame.
#[derive(Debug, Clone)]
pub enum EcsOp {
    Set {
        entity: Entity,
        type_path: String,
        patch: EcsValue,
    },
    Insert {
        entity: Entity,
        type_path: String,
        patch: EcsValue,
    },
    Remove {
        entity: Entity,
        type_path: String,
    },
    Spawn {
        entity: Entity,
        components: Vec<(String, EcsValue)>,
    },
    Despawn {
        entity: Entity,
    },
}

/// Espelho por frame dos componentes da [`WHITELIST`] (`bits → valor`).
#[derive(Debug, Default, Clone)]
pub struct EcsMirrorData {
    by_type: HashMap<&'static str, HashMap<i64, EcsValue>>,
}

impl EcsMirrorData {
    /// Valor refletido de `entity` em `type_path`, se o componente existir.
    pub fn get(&self, type_path: &str, bits: i64) -> Option<&EcsValue> {
        self.by_type.get(type_path)?.get(&bits)
    }

    /// Ids (bits) das entidades com `type_path`, ordem crescente.
    pub fn ids(&self, type_path: &str) -> Vec<i64> {
        let Some(map) = self.by_type.get(type_path) else {
            return Vec::new();
        };
        let mut ids: Vec<i64> = map.keys().copied().collect();
        ids.sort_unstable();
        ids
    }

    /// Tipos efetivamente com pelo menos uma entidade neste frame.
    pub fn present_types(&self) -> impl Iterator<Item = &&'static str> {
        self.by_type
            .iter()
            .filter(|(_, map)| !map.is_empty())
            .map(|(path, _)| path)
    }
}

/// Recurso com o espelho do frame (o `Arc` é clonado para o [`ScriptCtx`]).
#[derive(Resource, Debug, Default, Clone)]
pub struct EcsMirror(pub Arc<EcsMirrorData>);

/// Pool partilhado de entidades vazias para `viber.ecs.spawn`.
///
/// As entidades são criadas por [`mirror_system`] (que tem `&mut World`) e
/// retiradas pela closure Lua; o `Arc<Mutex<..>>` é o mesmo no recurso e no
/// [`ScriptCtx`]. Sem ele, `spawn` teria de devolver um id reservado (ou `nil`)
/// até os comandos aplicarem.
#[derive(Resource, Debug, Default, Clone)]
pub struct EcsSpawnPool {
    /// Ids livres (já `spawn_empty` no mundo).
    pub queue: Arc<Mutex<Vec<Entity>>>,
    /// Tamanho da fila no último refill (para medir o consumo do frame).
    filled_last: usize,
    /// Maior consumo observado — o refill seguinte cresce para 2×.
    high_water: usize,
}

impl EcsSpawnPool {
    /// Pool novo com a fila partilhável (o [`LuaScriptHost`] guarda um clone).
    pub fn new(queue: Arc<Mutex<Vec<Entity>>>) -> Self {
        Self {
            queue,
            filled_last: 0,
            high_water: 0,
        }
    }

    /// Tira um id livre. `None` = o orçamento deste frame acabou.
    pub fn take(&self) -> Option<Entity> {
        self.queue
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .pop()
    }
}

// ------------------------------------------------------------------ registo

/// `AppTypeRegistry` do mundo (clone do `Arc`; sem emprestar o `World`).
pub fn registry_arc(world: &World) -> Result<bevy::reflect::TypeRegistryArc, String> {
    world
        .get_resource::<bevy::ecs::reflect::AppTypeRegistry>()
        .map(|registry| registry.0.clone())
        .ok_or_else(|| "AppTypeRegistry ausente — o ReflectPlugin não está instalado".to_string())
}

fn registration<'a>(
    registry: &'a TypeRegistry,
    type_path: &str,
) -> Result<&'a TypeRegistration, String> {
    registry.get_with_type_path(type_path).ok_or_else(|| {
        format!("tipo '{type_path}' não está no AppTypeRegistry (vê viber.ecs.types())")
    })
}

fn component_reflection<'a>(
    registry: &'a TypeRegistry,
    type_path: &str,
) -> Result<&'a ReflectComponent, String> {
    let registration = registration(registry, type_path)?;
    registration
        .data::<ReflectComponent>()
        .ok_or_else(|| format!("'{type_path}' está registado mas não é um componente refletido"))
}

// ------------------------------------------------------------- leitura

/// Converte um valor refletido para [`EcsValue`] (structs um nível abaixo).
pub fn value_from_reflect(value: &dyn PartialReflect) -> EcsValue {
    value_from_reflect_at(value, 0)
}

const MAX_STRUCT_DEPTH: usize = 2;

fn value_from_reflect_at(value: &dyn PartialReflect, depth: usize) -> EcsValue {
    // Tipos com representação canónica na API Lua.
    if let Some(entity) = value.try_downcast_ref::<Entity>() {
        return EcsValue::Entity(entity.to_bits() as i64);
    }
    if let Some(vec) = value.try_downcast_ref::<Vec2>() {
        return EcsValue::Vec2([vec.x, vec.y]);
    }
    if let Some(vec) = value.try_downcast_ref::<Vec3>() {
        return EcsValue::Vec3([vec.x, vec.y, vec.z]);
    }
    if let Some(quat) = value.try_downcast_ref::<Quat>() {
        let [x, y, z, w] = quat.to_array();
        return EcsValue::Quat([x, y, z, w]);
    }
    if let Some(color) = value.try_downcast_ref::<Color>() {
        let color = color.to_srgba();
        return EcsValue::Color([color.red, color.green, color.blue, color.alpha]);
    }
    // Escalares.
    if let Some(value) = value.try_downcast_ref::<bool>() {
        return EcsValue::Bool(*value);
    }
    macro_rules! scalar {
        ($($ty:ty => $variant:ident),* $(,)?) => {$(
            if let Some(value) = value.try_downcast_ref::<$ty>() {
                return EcsValue::$variant((*value) as _);
            }
        )*};
    }
    scalar!(
        i8 => Int, i16 => Int, i32 => Int, i64 => Int, isize => Int,
        u8 => UInt, u16 => UInt, u32 => UInt, u64 => UInt, usize => UInt,
        f32 => Float, f64 => Float,
    );
    if let Some(value) = value.try_downcast_ref::<String>() {
        return EcsValue::Str(value.clone());
    }

    match value.reflect_ref() {
        ReflectRef::Struct(struct_value) => {
            if depth >= MAX_STRUCT_DEPTH {
                return EcsValue::Str(format!("<{}>", value.reflect_type_path()));
            }
            let mut fields = Vec::with_capacity(struct_value.field_len());
            for (name, field) in struct_value.iter_fields() {
                fields.push((name.to_string(), value_from_reflect_at(field, depth + 1)));
            }
            EcsValue::Map(fields)
        }
        ReflectRef::TupleStruct(tuple) => {
            if tuple.field_len() == 0 {
                return EcsValue::Unit;
            }
            EcsValue::Seq(
                (0..tuple.field_len())
                    .filter_map(|index| tuple.field(index))
                    .map(|field| value_from_reflect_at(field, depth + 1))
                    .collect(),
            )
        }
        ReflectRef::Tuple(tuple) => EcsValue::Seq(
            (0..tuple.field_len())
                .filter_map(|index| tuple.field(index))
                .map(|field| value_from_reflect_at(field, depth + 1))
                .collect(),
        ),
        ReflectRef::List(list) => EcsValue::Seq(
            (0..list.len())
                .filter_map(|index| list.get(index))
                .map(|field| value_from_reflect_at(field, depth + 1))
                .collect(),
        ),
        ReflectRef::Array(array) => EcsValue::Seq(
            (0..array.len())
                .filter_map(|index| array.get(index))
                .map(|field| value_from_reflect_at(field, depth + 1))
                .collect(),
        ),
        ReflectRef::Enum(enum_value) => {
            let name = enum_value.variant_name();
            if name == "None" && enum_value.field_len() == 0 {
                return EcsValue::Nil;
            }
            if name == "Some"
                && let Some(field) = enum_value.field_at(0)
            {
                return value_from_reflect_at(field, depth + 1);
            }
            if enum_value.variant_type() == VariantType::Unit {
                return EcsValue::Enum(name.to_string());
            }
            let mut fields = vec![("variant".to_string(), EcsValue::Enum(name.to_string()))];
            for (index, field) in enum_value.iter_fields().enumerate() {
                fields.push((
                    index.to_string(),
                    value_from_reflect_at(field.value(), depth + 1),
                ));
            }
            EcsValue::Map(fields)
        }
        // Mapas/sets/opacos não têm forma canónica nesta API (um script lê o
        // que é dado de jogo; caches são explicitamente para ficar de fora).
        _ => EcsValue::Str(format!("<{}>", value.reflect_type_path())),
    }
}

/// Constrói a tabela Lua de um valor refletido (`nil` para ausência).
fn value_to_lua(lua: &Lua, value: &EcsValue) -> mlua::Result<Value> {
    Ok(match value {
        EcsValue::Unit => Value::Table(lua.create_table()?),
        EcsValue::Nil => Value::Nil,
        EcsValue::Bool(value) => Value::Boolean(*value),
        EcsValue::Int(value) => (*value).into_lua(lua)?,
        EcsValue::UInt(value) => (*value).into_lua(lua)?,
        EcsValue::Float(value) => (*value).into_lua(lua)?,
        EcsValue::Str(value) | EcsValue::Enum(value) => Value::String(lua.create_string(value)?),
        EcsValue::Entity(bits) => (*bits).into_lua(lua)?,
        EcsValue::Vec2([x, y]) => {
            let table = lua.create_table()?;
            table.raw_set("x", *x)?;
            table.raw_set("y", *y)?;
            Value::Table(table)
        }
        EcsValue::Vec3([x, y, z]) => {
            let table = lua.create_table()?;
            table.raw_set("x", *x)?;
            table.raw_set("y", *y)?;
            table.raw_set("z", *z)?;
            Value::Table(table)
        }
        EcsValue::Quat([x, y, z, w]) => {
            let table = lua.create_table()?;
            table.raw_set("x", *x)?;
            table.raw_set("y", *y)?;
            table.raw_set("z", *z)?;
            table.raw_set("w", *w)?;
            Value::Table(table)
        }
        EcsValue::Color([r, g, b, a]) => {
            let table = lua.create_table()?;
            table.raw_set("r", *r)?;
            table.raw_set("g", *g)?;
            table.raw_set("b", *b)?;
            table.raw_set("a", *a)?;
            Value::Table(table)
        }
        EcsValue::Map(fields) => {
            let table = lua.create_table()?;
            for (name, value) in fields {
                // Um campo `nil` não existe em Lua: omite-se (mesma leitura
                // que `Option::None`).
                if matches!(value, EcsValue::Nil) {
                    continue;
                }
                table.raw_set(name.as_str(), value_to_lua(lua, value)?)?;
            }
            Value::Table(table)
        }
        EcsValue::Seq(values) => {
            let table = lua.create_table()?;
            for (index, value) in values.iter().enumerate() {
                table.raw_set(index + 1, value_to_lua(lua, value)?)?;
            }
            Value::Table(table)
        }
    })
}

/// Converte uma tabela Lua num [`EcsValue`] (sequência `1..n` ou mapa nomeado).
fn value_from_lua(value: Value) -> mlua::Result<EcsValue> {
    Ok(match value {
        Value::Nil => EcsValue::Nil,
        Value::Boolean(value) => EcsValue::Bool(value),
        Value::Integer(value) => EcsValue::Int(value as i64),
        Value::Number(value) => EcsValue::Float(value),
        Value::String(value) => EcsValue::Str(value.to_str()?.to_string()),
        Value::Table(table) => table_from_lua(&table)?,
        other => {
            return Err(mlua::Error::runtime(format!(
                "viber.ecs: valor '{}' não suportado (escalares, booleanos, strings, \
                 {{x=,y=,z=}} ou arrays)",
                other.type_name()
            )));
        }
    })
}

fn table_from_lua(table: &Table) -> mlua::Result<EcsValue> {
    let len = table.raw_len();
    let mut count = 0usize;
    let mut is_sequence = len > 0;
    for pair in table.clone().pairs::<Value, Value>() {
        let (key, _) = pair?;
        count += 1;
        match key {
            Value::Integer(index) if index >= 1 && (index as usize) <= len => {}
            _ => is_sequence = false,
        }
    }
    if is_sequence && count == len {
        let mut values = Vec::with_capacity(len);
        for index in 1..=len {
            values.push(value_from_lua(table.raw_get(index)?)?);
        }
        return Ok(EcsValue::Seq(values));
    }
    let mut fields = Vec::with_capacity(count);
    for pair in table.clone().pairs::<Value, Value>() {
        let (key, value) = pair?;
        let name = match key {
            Value::String(name) => name.to_str()?.to_string(),
            // Campos de variantes tuplo (`{variant="X", "0"=…}`).
            Value::Integer(index) => index.to_string(),
            other => {
                return Err(mlua::Error::runtime(format!(
                    "viber.ecs: chave de tabela '{}' não suportada (usa nomes)",
                    other.type_name()
                )));
            }
        };
        fields.push((name, value_from_lua(value)?));
    }
    // Ordem determinística (a iteração de `pairs` não é).
    fields.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(EcsValue::Map(fields))
}

// ------------------------------------------------------------- escrita

/// Resultado de um patch: o componente existia? (`false` = entidade sem ele).
pub type Patched = bool;

/// Aplica um patch parcial a um componente já existente.
///
/// `type_path` não é validado contra a [`WHITELIST`]: os verbos internos
/// (`set_pose`/`set_scale`/`set_visible`) usam-no para `Transform`/`Visibility`.
pub fn patch_component(
    world: &mut World,
    entity: Entity,
    type_path: &str,
    patch: &EcsValue,
) -> Result<Patched, String> {
    let registry = registry_arc(world)?;
    let registry = registry.read();
    let reflection = component_reflection(&registry, type_path)?;
    let Ok(mut entity_mut) = world.get_entity_mut(entity) else {
        return Err(format!("entidade {entity:?} não existe"));
    };
    let Some(mut value) = reflection.reflect_mut(&mut entity_mut) else {
        return Ok(false);
    };
    match value.reflect_mut() {
        ReflectMut::Struct(struct_value) => {
            let fields = match patch {
                EcsValue::Unit | EcsValue::Nil => return Ok(true),
                EcsValue::Map(fields) => fields,
                _ => {
                    return Err(format!(
                        "{type_path}: o patch tem de ser uma tabela {{campo = valor}}"
                    ));
                }
            };
            for (name, field_value) in fields {
                let field = struct_value
                    .field_mut(name)
                    .ok_or_else(|| format!("{type_path}: campo '{name}' não existe"))?;
                set_from_value(
                    field,
                    field_value,
                    &format!("{type_path}.{name}"),
                    &registry,
                )?;
            }
        }
        // Componente tuplo (`BaseFov`, `Groups`): vazio é no-op; um array
        // posicional escreve os campos por índice.
        ReflectMut::TupleStruct(tuple) => match patch {
            EcsValue::Unit | EcsValue::Nil => {}
            EcsValue::Map(fields) if fields.is_empty() => {}
            EcsValue::Seq(values) => {
                for (index, field_value) in values.iter().enumerate() {
                    let field = tuple
                        .field_mut(index)
                        .ok_or_else(|| format!("{type_path}: campo {index} não existe"))?;
                    set_from_value(
                        field,
                        field_value,
                        &format!("{type_path}.{index}"),
                        &registry,
                    )?;
                }
            }
            _ => {
                return Err(format!(
                    "{type_path}: componente tuplo espera um array posicional"
                ));
            }
        },
        // Componente que é ele próprio um enum (`Visibility`): o patch é a
        // variante (`"Hidden"`) ou o payload dela.
        ReflectMut::Enum(_) => {
            set_from_value(&mut *value, patch, type_path, &registry)?;
        }
        _ => {
            return Err(format!(
                "{type_path}: tipo {} não suportado por viber.ecs",
                value.reflect_type_path()
            ));
        }
    }
    Ok(true)
}

/// Insere (ou substitui) um componente construído por reflexão.
pub fn insert_component(
    world: &mut World,
    entity: Entity,
    type_path: &str,
    patch: &EcsValue,
) -> Result<(), String> {
    let registry = registry_arc(world)?;
    let registry = registry.read();
    let reflection = component_reflection(&registry, type_path)?;
    let value = build_component(&registry, type_path, patch)?;
    let Ok(mut entity_mut) = world.get_entity_mut(entity) else {
        return Err(format!("entidade {entity:?} não existe"));
    };
    reflection.insert(&mut entity_mut, value.as_partial_reflect(), &registry);
    Ok(())
}

/// Remove um componente; `false` se a entidade não o tinha.
pub fn remove_component(
    world: &mut World,
    entity: Entity,
    type_path: &str,
) -> Result<bool, String> {
    let registry = registry_arc(world)?;
    let registry = registry.read();
    let reflection = component_reflection(&registry, type_path)?;
    let Ok(mut entity_mut) = world.get_entity_mut(entity) else {
        return Err(format!("entidade {entity:?} não existe"));
    };
    if !reflection.contains(&entity_mut) {
        return Ok(false);
    }
    reflection.remove(&mut entity_mut);
    Ok(true)
}

/// Aplica um [`EcsOp`] enfileirado por um script (pós-frame).
pub fn apply_op(world: &mut World, op: &EcsOp) -> Result<(), String> {
    match op {
        EcsOp::Set {
            entity,
            type_path,
            patch,
        } => {
            if patch_component(world, *entity, type_path, patch)? {
                Ok(())
            } else {
                Err(format!(
                    "viber.ecs.set: entidade {:?} não tem '{type_path}' (usa insert)",
                    entity
                ))
            }
        }
        EcsOp::Insert {
            entity,
            type_path,
            patch,
        } => insert_component(world, *entity, type_path, patch),
        EcsOp::Remove { entity, type_path } => {
            if remove_component(world, *entity, type_path)? {
                Ok(())
            } else {
                Err(format!(
                    "viber.ecs.remove: entidade {:?} não tem '{type_path}'",
                    entity
                ))
            }
        }
        EcsOp::Spawn { entity, components } => {
            if world.get_entity(*entity).is_err() {
                return Err(format!(
                    "viber.ecs.spawn: o id reservado {entity:?} já não existe"
                ));
            }
            for (type_path, patch) in components {
                if let Err(error) = insert_component(world, *entity, type_path, patch) {
                    // Spawn parcial não fica no mundo: o script recebeu um id
                    // que nunca teria componentes completos.
                    world.despawn(*entity);
                    return Err(error);
                }
            }
            Ok(())
        }
        EcsOp::Despawn { entity } => {
            world.despawn(*entity);
            Ok(())
        }
    }
}

// -------------------------------------------------- construir um componente

/// Constrói o valor concreto de `type_path` a partir do patch Lua.
///
/// Preferência: `Default` refletido (o tipo decide os campos ausentes) e só
/// depois `FromReflect` com TODOS os campos fornecidos. Um tipo sem `Default`
/// nem todos os campos devolve erro claro — nunca uma construção a zeros.
fn build_component(
    registry: &TypeRegistry,
    type_path: &str,
    patch: &EcsValue,
) -> Result<Box<dyn Reflect>, String> {
    let registration = registration(registry, type_path)?;
    let info = registration.type_info();

    // Caminho 1: Default do próprio tipo (a forma segura para `Health`,
    // `Player`, `AiLocomotion`, …).
    if let Some(default) = registration.data::<ReflectDefault>() {
        let mut value = default.default();
        apply_patch_to_value(value.as_partial_reflect_mut(), patch, type_path, registry)?;
        return Ok(value);
    }

    // Caminho 2: `FromReflect` exige todos os campos ativos.
    let struct_info = info
        .as_struct()
        .map_err(|_| format!("{type_path}: insert de viber.ecs só suporta componentes struct"))?;
    let fields = match patch {
        EcsValue::Unit | EcsValue::Nil => Vec::new(),
        EcsValue::Map(fields) => fields.clone(),
        _ => {
            return Err(format!(
                "{type_path}: o patch de insert tem de ser uma tabela {{campo = valor}}"
            ));
        }
    };
    for (name, value) in &fields {
        let field = struct_info
            .field(name)
            .ok_or_else(|| format!("{type_path}: campo '{name}' não existe"))?;
        build_partial(field.type_info(), value, &format!("{type_path}.{name}")).map(|_| ())?;
    }
    let missing: Vec<&str> = struct_info
        .field_names()
        .iter()
        .copied()
        .filter(|name| {
            let wanted: &str = *name;
            !fields.iter().any(|(field, _)| field.as_str() == wanted)
        })
        .collect();
    if !missing.is_empty() {
        return Err(format!(
            "{type_path}: tipo sem Default de reflexão e faltam campos {missing:?} \
             (fornece-os todos no insert)"
        ));
    }
    let from_reflect = registration.data::<ReflectFromReflect>().ok_or_else(|| {
        format!("{type_path}: não é construível por reflexão (sem FromReflect/Default)")
    })?;
    let mut dynamic = DynamicStruct::default();
    dynamic.set_represented_type(Some(info));
    for (name, value) in &fields {
        let field = struct_info
            .field(name)
            .ok_or_else(|| format!("{type_path}: campo '{name}' não existe"))?;
        dynamic.insert_boxed(
            name.clone(),
            build_partial(field.type_info(), value, &format!("{type_path}.{name}"))?,
        );
    }
    from_reflect
        .from_reflect(&dynamic)
        .ok_or_else(|| format!("{type_path}: FromReflect recusou o valor construído"))
}

/// Aplica os campos de um `Map` a um valor concreto já existente.
fn apply_patch_to_value(
    value: &mut dyn bevy::reflect::PartialReflect,
    patch: &EcsValue,
    path: &str,
    registry: &TypeRegistry,
) -> Result<(), String> {
    match patch {
        EcsValue::Unit | EcsValue::Nil => Ok(()),
        EcsValue::Map(fields) if fields.is_empty() => Ok(()),
        EcsValue::Map(fields) => {
            let Ok(struct_value) = value.reflect_mut().as_struct() else {
                return Err(format!(
                    "{path}: o componente não é um struct — passa {{}} ou os campos"
                ));
            };
            for (name, field_value) in fields {
                let field = struct_value
                    .field_mut(name)
                    .ok_or_else(|| format!("{path}: campo '{name}' não existe"))?;
                set_from_value(field, field_value, &format!("{path}.{name}"), registry)?;
            }
            Ok(())
        }
        _ => Err(format!(
            "{path}: o patch tem de ser uma tabela {{campo = valor}}"
        )),
    }
}

/// Constrói um valor dinâmico de um campo a partir do `TypeInfo` do campo.
///
/// Só é usado no caminho sem `Default` (todos os campos são fornecidos).
fn build_partial(
    info: Option<&'static TypeInfo>,
    value: &EcsValue,
    path: &str,
) -> Result<Box<dyn bevy::reflect::PartialReflect>, String> {
    let info = info.ok_or_else(|| format!("{path}: tipo sem TypeInfo (opaco)"))?;
    // Os canónicos de vetor/cor viajam como variantes próprias e são
    // construídos pelo `TypeId` (o `TypeInfo` deles pode ser Struct).
    if matches!(
        value,
        EcsValue::Vec2(_) | EcsValue::Vec3(_) | EcsValue::Quat(_) | EcsValue::Color(_)
    ) {
        return build_opaque(info, value, path);
    }
    match info {
        TypeInfo::Opaque(_) => build_opaque(info, value, path),
        TypeInfo::Struct(struct_info) => {
            let fields = match value {
                EcsValue::Unit | EcsValue::Nil => &[][..],
                EcsValue::Map(fields) => fields.as_slice(),
                _ => return Err(format!("{path}: esperava uma tabela de campos")),
            };
            let mut dynamic = DynamicStruct::default();
            dynamic.set_represented_type(Some(info));
            for (name, field_value) in fields {
                let field = struct_info
                    .field(name)
                    .ok_or_else(|| format!("{path}: campo '{name}' não existe"))?;
                dynamic.insert_boxed(
                    name.clone(),
                    build_partial(field.type_info(), field_value, &format!("{path}.{name}"))?,
                );
            }
            Ok(Box::new(dynamic))
        }
        TypeInfo::Enum(enum_info) => {
            // `Option<T>`: payload direto (`{x,y}`, bits de entidade, …) →
            // `Some`; `nil`/`"none"` → `None`.
            if enum_info.variant("Some").is_some() && enum_info.variant("None").is_some() {
                if matches!(value, EcsValue::Nil)
                    || matches!(value, EcsValue::Str(name) if name.eq_ignore_ascii_case("none"))
                {
                    let index = enum_info.index_of("None").unwrap_or(0);
                    let mut dynamic =
                        DynamicEnum::new_with_index(index, "None", DynamicVariant::Unit);
                    dynamic.set_represented_type(Some(info));
                    return Ok(Box::new(dynamic));
                }
                let variant = enum_info
                    .variant("Some")
                    .ok_or_else(|| format!("{path}: variante Some ausente"))?;
                let tuple_info = variant
                    .as_tuple_variant()
                    .map_err(|_| format!("{path}: variante Some não é tuplo"))?;
                let field = tuple_info
                    .field_at(0)
                    .ok_or_else(|| format!("{path}: Some sem campo"))?;
                let mut tuple = DynamicTuple::default();
                tuple.insert_boxed(build_partial(field.type_info(), value, path)?);
                let index = enum_info.index_of("Some").unwrap_or(0);
                let mut dynamic =
                    DynamicEnum::new_with_index(index, "Some", DynamicVariant::Tuple(tuple));
                dynamic.set_represented_type(Some(info));
                return Ok(Box::new(dynamic));
            }
            let (name, payload) = match value {
                EcsValue::Str(name) => (name.as_str(), None),
                EcsValue::Enum(name) => (name.as_str(), None),
                EcsValue::Nil => ("None", None),
                EcsValue::Map(fields) => {
                    let Some((_, EcsValue::Str(name) | EcsValue::Enum(name))) =
                        fields.iter().find(|(key, _)| key == "variant")
                    else {
                        return Err(format!(
                            "{path}: variante de enum precisa de {{variant = \"Nome\"}}"
                        ));
                    };
                    (name.as_str(), Some(value))
                }
                _ => {
                    return Err(format!("{path}: enum espera o nome da variante (string)"));
                }
            };
            let variant = enum_info.variant(name).ok_or_else(|| {
                format!(
                    "{path}: variante '{name}' não existe em {}",
                    info.type_path()
                )
            })?;
            let index = enum_info.index_of(name).ok_or_else(|| {
                format!(
                    "{path}: variante '{name}' não existe em {}",
                    info.type_path()
                )
            })?;
            let dyn_variant = match variant.variant_type() {
                VariantType::Unit => DynamicVariant::Unit,
                VariantType::Tuple => {
                    let tuple_info = variant
                        .as_tuple_variant()
                        .map_err(|_| format!("{path}: variante '{name}' não tem campos"))?;
                    let mut tuple = DynamicTuple::default();
                    let Some(payload) = payload else {
                        return Err(format!(
                            "{path}: variante '{name}' tem payload — usa \
                             {{variant = \"{name}\", \"0\" = …}}"
                        ));
                    };
                    let field = tuple_info.field_at(0).ok_or_else(|| {
                        format!("{path}: variante '{name}' sem campos no TypeInfo")
                    })?;
                    let field_value = field_from_variant_table(payload)?;
                    tuple.insert_boxed(build_partial(
                        field.type_info(),
                        field_value,
                        &format!("{path}.{name}.0"),
                    )?);
                    DynamicVariant::Tuple(tuple)
                }
                VariantType::Struct => {
                    let struct_info = variant
                        .as_struct_variant()
                        .map_err(|_| format!("{path}: variante '{name}' não tem campos"))?;
                    let mut dynamic = DynamicStruct::default();
                    if let Some(EcsValue::Map(fields)) = payload {
                        for (field_name, field_value) in fields {
                            if field_name == "variant" {
                                continue;
                            }
                            let field = struct_info.field(field_name).ok_or_else(|| {
                                format!("{path}.{name}: campo '{field_name}' não existe")
                            })?;
                            dynamic.insert_boxed(
                                field_name.clone(),
                                build_partial(
                                    field.type_info(),
                                    field_value,
                                    &format!("{path}.{name}.{field_name}"),
                                )?,
                            );
                        }
                    }
                    DynamicVariant::Struct(dynamic)
                }
            };
            let mut dynamic = DynamicEnum::new_with_index(index, name, dyn_variant);
            dynamic.set_represented_type(Some(info));
            Ok(Box::new(dynamic))
        }
        TypeInfo::TupleStruct(tuple_info) => {
            let values = match value {
                EcsValue::Seq(values) => values.clone(),
                _ => return Err(format!("{path}: esperava um array posicional")),
            };
            let mut dynamic = DynamicTupleStruct::default();
            dynamic.set_represented_type(Some(info));
            for (index, field_value) in values.iter().enumerate() {
                let field = tuple_info
                    .field_at(index)
                    .ok_or_else(|| format!("{path}: campo {index} não existe"))?;
                dynamic.insert_boxed(build_partial(
                    field.type_info(),
                    field_value,
                    &format!("{path}.{index}"),
                )?);
            }
            Ok(Box::new(dynamic))
        }
        _ => Err(format!(
            "{path}: tipo {} não construível por viber.ecs.insert",
            info.type_path()
        )),
    }
}

/// Payload de uma variante tuplo a partir da tabela `{variant="X", "0"=…}`.
fn field_from_variant_table(value: &EcsValue) -> Result<&EcsValue, String> {
    match value {
        EcsValue::Map(fields) => fields
            .iter()
            .find(|(name, _)| name == "0")
            .map(|(_, value)| value)
            .ok_or_else(|| "variante tuplo sem o campo \"0\"".to_string()),
        _ => Err("variante tuplo espera uma tabela {variant=…, \"0\"=…}".to_string()),
    }
}

/// Constrói um tipo "opaco" (primitivos e vetores da Bevy) a partir do valor.
fn build_opaque(
    info: &'static TypeInfo,
    value: &EcsValue,
    path: &str,
) -> Result<Box<dyn bevy::reflect::PartialReflect>, String> {
    use std::any::TypeId;
    let type_id = info.type_id();
    macro_rules! number {
        ($($ty:ty),* $(,)?) => {$(
            if type_id == TypeId::of::<$ty>() {
                let number = float_value(value, path)?;
                return Ok(Box::new(number as $ty));
            }
        )*};
    }
    macro_rules! integer {
        ($($ty:ty),* $(,)?) => {$(
            if type_id == TypeId::of::<$ty>() {
                let number = int_value(value, path)?;
                return Ok(Box::new(<$ty>::try_from(number).map_err(|_| {
                    format!("{path}: {number} fora do alcance de {}", stringify!($ty))
                })?));
            }
        )*};
    }
    if type_id == TypeId::of::<bool>() {
        return Ok(Box::new(match value {
            EcsValue::Bool(value) => *value,
            _ => return Err(format!("{path}: esperava um booleano")),
        }));
    }
    if type_id == TypeId::of::<String>() {
        return Ok(Box::new(match value {
            EcsValue::Str(value) => value.clone(),
            _ => return Err(format!("{path}: esperava uma string")),
        }));
    }
    integer!(i8, i16, i32, i64, isize);
    integer!(u8, u16, u32, u64, usize);
    number!(f32, f64);
    if type_id == TypeId::of::<Vec2>() {
        let [x, y] = vec_components::<2>(value, path)?;
        return Ok(Box::new(Vec2::new(x, y)));
    }
    if type_id == TypeId::of::<Vec3>() {
        let [x, y, z] = vec_components::<3>(value, path)?;
        return Ok(Box::new(Vec3::new(x, y, z)));
    }
    if type_id == TypeId::of::<Quat>() {
        let [x, y, z, w] = vec_components::<4>(value, path)?;
        return Ok(Box::new(Quat::from_xyzw(x, y, z, w)));
    }
    if type_id == TypeId::of::<Color>() {
        let [r, g, b, a] = color_components(value, path)?;
        return Ok(Box::new(Color::srgba(r, g, b, a)));
    }
    if type_id == TypeId::of::<Entity>() {
        let bits = int_value(value, path)?;
        let entity = Entity::try_from_bits(bits as u64)
            .ok_or_else(|| format!("{path}: id de entidade inválido ({bits})"))?;
        return Ok(Box::new(entity));
    }
    Err(format!(
        "{path}: tipo '{}' não construível por viber.ecs.insert",
        info.type_path()
    ))
}

// ------------------------------------------------------- atribuir um valor

/// Atribui `value` a um campo concreto, convertendo para o tipo do campo.
fn set_from_value(
    target: &mut dyn bevy::reflect::PartialReflect,
    value: &EcsValue,
    path: &str,
    registry: &TypeRegistry,
) -> Result<(), String> {
    macro_rules! integer {
        ($($ty:ty),* $(,)?) => {$(
            if let Some(slot) = target.try_downcast_mut::<$ty>() {
                let number = int_value(value, path)?;
                *slot = <$ty>::try_from(number).map_err(|_| {
                    format!("{path}: {number} fora do alcance de {}", stringify!($ty))
                })?;
                return Ok(());
            }
        )*};
    }
    macro_rules! unsigned {
        ($($ty:ty),* $(,)?) => {$(
            if let Some(slot) = target.try_downcast_mut::<$ty>() {
                let number = uint_value(value, path)?;
                *slot = <$ty>::try_from(number).map_err(|_| {
                    format!("{path}: {number} fora do alcance de {}", stringify!($ty))
                })?;
                return Ok(());
            }
        )*};
    }
    macro_rules! float {
        ($($ty:ty),* $(,)?) => {$(
            if let Some(slot) = target.try_downcast_mut::<$ty>() {
                *slot = float_value(value, path)? as $ty;
                return Ok(());
            }
        )*};
    }

    if let Some(slot) = target.try_downcast_mut::<bool>() {
        *slot = match value {
            EcsValue::Bool(value) => *value,
            _ => return Err(format!("{path}: esperava um booleano")),
        };
        return Ok(());
    }
    integer!(i8, i16, i32, i64, isize);
    unsigned!(u8, u16, u32, u64, usize);
    float!(f32, f64);
    if let Some(slot) = target.try_downcast_mut::<String>() {
        *slot = match value {
            EcsValue::Str(value) => value.clone(),
            _ => return Err(format!("{path}: esperava uma string")),
        };
        return Ok(());
    }
    if let Some(slot) = target.try_downcast_mut::<Vec2>() {
        let [x, y] = vec_components::<2>(value, path)?;
        *slot = Vec2::new(x, y);
        return Ok(());
    }
    if let Some(slot) = target.try_downcast_mut::<Vec3>() {
        let [x, y, z] = vec_components::<3>(value, path)?;
        *slot = Vec3::new(x, y, z);
        return Ok(());
    }
    if let Some(slot) = target.try_downcast_mut::<Quat>() {
        let [x, y, z, w] = vec_components::<4>(value, path)?;
        *slot = Quat::from_xyzw(x, y, z, w);
        return Ok(());
    }
    if let Some(slot) = target.try_downcast_mut::<Color>() {
        let [r, g, b, a] = color_components(value, path)?;
        *slot = Color::srgba(r, g, b, a);
        return Ok(());
    }
    if let Some(slot) = target.try_downcast_mut::<Entity>() {
        let bits = int_value(value, path)?;
        *slot = Entity::try_from_bits(bits as u64)
            .ok_or_else(|| format!("{path}: id de entidade inválido ({bits})"))?;
        return Ok(());
    }

    // Enums e structs aninhados.
    let type_path = target.reflect_type_path().to_string();
    let enum_info = target.get_represented_type_info().and_then(|info| {
        if let TypeInfo::Enum(enum_info) = info {
            Some(enum_info)
        } else {
            None
        }
    });
    if let Some(enum_info) = enum_info {
        return set_enum_from_value(target, enum_info, value, path, registry);
    }
    if target.reflect_mut().as_struct().is_ok() {
        match value {
            EcsValue::Map(_) | EcsValue::Unit | EcsValue::Nil => {
                return apply_patch_to_value(target, value, path, registry);
            }
            _ => {
                return Err(format!(
                    "{path}: '{type_path}' é um struct — esperava uma tabela de campos"
                ));
            }
        }
    }
    Err(format!(
        "{path}: tipo '{type_path}' não suportado por viber.ecs"
    ))
}

/// Atribui uma variante a um campo enum (`Visibility`, `Option`, `Faction`, …).
fn set_enum_from_value(
    target: &mut dyn bevy::reflect::PartialReflect,
    enum_info: &'static EnumInfo,
    value: &EcsValue,
    path: &str,
    registry: &TypeRegistry,
) -> Result<(), String> {
    let represented = target.get_represented_type_info();
    let current = target
        .reflect_ref()
        .as_enum()
        .map(Enum::variant_name)
        .unwrap_or_default()
        .to_string();

    // `Option<T>`: o `Some` atual recebe o valor; o `None` passa a `Some`
    // (payload construído do `TypeInfo` do campo 0); `nil`/`"none"` → `None`.
    if enum_info.variant("Some").is_some() && enum_info.variant("None").is_some() {
        if matches!(value, EcsValue::Nil)
            || matches!(value, EcsValue::Str(name) if name.eq_ignore_ascii_case("none"))
        {
            let none_index = enum_info.index_of("None").unwrap_or(0);
            let mut dynamic = DynamicEnum::new_with_index(none_index, "None", DynamicVariant::Unit);
            dynamic.set_represented_type(represented);
            return target
                .reflect_mut()
                .as_enum()
                .expect("enum")
                .try_apply(&dynamic)
                .map_err(|error| format!("{path}: {error}"));
        }
        if current == "Some" {
            let enum_mut = target.reflect_mut().as_enum().expect("enum");
            let field = enum_mut
                .field_at_mut(0)
                .ok_or_else(|| format!("{path}: Option::Some sem payload"))?;
            return set_from_value(field, value, path, registry);
        }
        let variant = enum_info
            .variant("Some")
            .ok_or_else(|| format!("{path}: variante Some ausente"))?;
        let tuple_info = variant
            .as_tuple_variant()
            .map_err(|_| format!("{path}: variante Some não é tuplo"))?;
        let field = tuple_info
            .field_at(0)
            .ok_or_else(|| format!("{path}: Some sem campo"))?;
        let payload = build_partial(field.type_info(), value, path)?;
        let mut tuple = DynamicTuple::default();
        tuple.insert_boxed(payload);
        let some_index = enum_info.index_of("Some").unwrap_or(0);
        let mut dynamic =
            DynamicEnum::new_with_index(some_index, "Some", DynamicVariant::Tuple(tuple));
        dynamic.set_represented_type(represented);
        return target
            .reflect_mut()
            .as_enum()
            .expect("enum")
            .try_apply(&dynamic)
            .map_err(|error| format!("{path}: {error}"));
    }

    let Some(name) = (match value {
        EcsValue::Str(name) => Some(name.as_str()),
        EcsValue::Enum(name) => Some(name.as_str()),
        _ => None,
    }) else {
        return Err(format!(
            "{path}: enum '{}' espera o nome da variante (string)",
            enum_info.type_path()
        ));
    };
    let variant = enum_info.variant(name).ok_or_else(|| {
        format!(
            "{path}: variante '{name}' não existe em {}",
            enum_info.type_path()
        )
    })?;
    let index = enum_info.index_of(name).unwrap_or(0);
    let dyn_variant = match variant.variant_type() {
        VariantType::Unit => DynamicVariant::Unit,
        _ => {
            return Err(format!(
                "{path}: variante '{name}' tem payload — passa uma tabela"
            ));
        }
    };
    let mut dynamic = DynamicEnum::new_with_index(index, name, dyn_variant);
    dynamic.set_represented_type(represented);
    target
        .reflect_mut()
        .as_enum()
        .expect("enum")
        .try_apply(&dynamic)
        .map_err(|error| format!("{path}: {error}"))
}

// --------------------------------------------------------------- conversões

fn float_value(value: &EcsValue, path: &str) -> Result<f64, String> {
    let number = match value {
        EcsValue::Float(value) => *value,
        EcsValue::Int(value) => *value as f64,
        EcsValue::UInt(value) => *value as f64,
        _ => {
            return Err(format!(
                "{path}: esperava um número (recebi {})",
                value_kind(value)
            ));
        }
    };
    if !number.is_finite() {
        return Err(format!("{path}: número não finito (NaN/inf)"));
    }
    Ok(number)
}

fn int_value(value: &EcsValue, path: &str) -> Result<i64, String> {
    match value {
        EcsValue::Int(value) => Ok(*value),
        EcsValue::Entity(bits) => Ok(*bits),
        EcsValue::UInt(value) => {
            i64::try_from(*value).map_err(|_| format!("{path}: {value} fora do alcance de i64"))
        }
        EcsValue::Float(value)
            if value.is_finite()
                && value.fract() == 0.0
                && *value >= i64::MIN as f64
                && *value <= i64::MAX as f64 =>
        {
            Ok(*value as i64)
        }
        _ => Err(format!(
            "{path}: esperava um inteiro (recebi {})",
            value_kind(value)
        )),
    }
}

fn uint_value(value: &EcsValue, path: &str) -> Result<u64, String> {
    match value {
        EcsValue::UInt(value) => Ok(*value),
        EcsValue::Entity(bits) if *bits >= 0 => Ok(*bits as u64),
        EcsValue::Int(value) if *value >= 0 => Ok(*value as u64),
        EcsValue::Float(value)
            if value.is_finite()
                && value.fract() == 0.0
                && *value >= 0.0
                && *value <= u64::MAX as f64 =>
        {
            Ok(*value as u64)
        }
        _ => Err(format!(
            "{path}: esperava um inteiro >= 0 (recebi {})",
            value_kind(value)
        )),
    }
}

fn vec_components<const N: usize>(value: &EcsValue, path: &str) -> Result<[f32; N], String> {
    let names: [&str; 4] = ["x", "y", "z", "w"];
    let mut out = [0.0f32; N];
    match value {
        EcsValue::Vec2(values) if N == 2 => {
            out.copy_from_slice(values);
            Ok(out)
        }
        EcsValue::Vec3(values) if N == 3 => {
            out.copy_from_slice(values);
            Ok(out)
        }
        EcsValue::Quat(values) if N == 4 => {
            out.copy_from_slice(values);
            Ok(out)
        }
        EcsValue::Seq(values) if values.len() == N => {
            for (index, item) in values.iter().enumerate() {
                out[index] = float_value(item, &format!("{path}[{}]", index + 1))? as f32;
            }
            Ok(out)
        }
        EcsValue::Map(fields) => {
            for index in 0..N {
                let name = names[index];
                let item = fields
                    .iter()
                    .find(|(key, _)| key == name)
                    .map(|(_, value)| value)
                    .ok_or_else(|| format!("{path}: falta '{name}'"))?;
                out[index] = float_value(item, &format!("{path}.{name}"))? as f32;
            }
            Ok(out)
        }
        _ => Err(format!(
            "{path}: esperava {{{},…}} ou um array de {N} números",
            names[..N].join(",")
        )),
    }
}

fn color_components(value: &EcsValue, path: &str) -> Result<[f32; 4], String> {
    match value {
        EcsValue::Color(values) => Ok(*values),
        EcsValue::Seq(values) if values.len() == 4 => {
            let mut out = [1.0f32; 4];
            for (index, item) in values.iter().enumerate() {
                out[index] = float_value(item, &format!("{path}[{}]", index + 1))? as f32;
            }
            Ok(out)
        }
        EcsValue::Map(fields) => {
            let mut out = [1.0f32; 4];
            let mut seen = [false; 3];
            for (key, item) in fields {
                let slot = match key.as_str() {
                    "r" => 0,
                    "g" => 1,
                    "b" => 2,
                    "a" => 3,
                    _ => continue,
                };
                out[slot] = float_value(item, &format!("{path}.{key}"))? as f32;
                if slot < 3 {
                    seen[slot] = true;
                }
            }
            if !(seen[0] && seen[1] && seen[2]) {
                return Err(format!("{path}: faltam canais r/g/b"));
            }
            Ok(out)
        }
        _ => Err(format!(
            "{path}: esperava {{r,g,b,a}} ou um array de 4 números"
        )),
    }
}

fn value_kind(value: &EcsValue) -> &'static str {
    match value {
        EcsValue::Unit => "tabela vazia",
        EcsValue::Nil => "nil",
        EcsValue::Bool(_) => "booleano",
        EcsValue::Int(_) | EcsValue::UInt(_) => "inteiro",
        EcsValue::Float(_) => "número",
        EcsValue::Str(_) | EcsValue::Enum(_) => "string",
        EcsValue::Vec2(_) | EcsValue::Vec3(_) | EcsValue::Quat(_) | EcsValue::Color(_) => "tabela",
        EcsValue::Entity(_) => "id de entidade",
        EcsValue::Map(_) | EcsValue::Seq(_) => "tabela",
    }
}

// ------------------------------------------------------- validação pré-fila

/// Valida um patch antes de o enfileirar (nomes de campo/variante e forma).
fn validate_patch(
    registry: &TypeRegistry,
    type_path: &str,
    patch: &EcsValue,
) -> Result<(), String> {
    let registration = registration(registry, type_path)?;
    let info = registration.type_info();
    match info {
        TypeInfo::Struct(struct_info) => {
            if let EcsValue::Map(fields) = patch {
                for (name, value) in fields {
                    let field = struct_info
                        .field(name)
                        .ok_or_else(|| format!("{type_path}: campo '{name}' não existe"))?;
                    validate_value(field.type_info(), value, &format!("{type_path}.{name}"))?;
                }
            }
            Ok(())
        }
        TypeInfo::Enum(enum_info) => validate_enum(enum_info, patch, type_path),
        _ => Ok(()),
    }
}

fn validate_value(
    info: Option<&'static TypeInfo>,
    value: &EcsValue,
    path: &str,
) -> Result<(), String> {
    let Some(info) = info else {
        return Ok(());
    };
    match info {
        TypeInfo::Enum(enum_info) => validate_enum(enum_info, value, path),
        TypeInfo::Struct(struct_info) => {
            if let EcsValue::Map(fields) = value {
                for (name, field_value) in fields {
                    let field = struct_info
                        .field(name)
                        .ok_or_else(|| format!("{path}: campo '{name}' não existe"))?;
                    validate_value(field.type_info(), field_value, &format!("{path}.{name}"))?;
                }
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn validate_enum(enum_info: &'static EnumInfo, value: &EcsValue, path: &str) -> Result<(), String> {
    let is_option = enum_info.variant("Some").is_some() && enum_info.variant("None").is_some();
    // `Option<T>` aceita o payload diretamente (`{x,y}`, `5`, `"none"`…) — o
    // `Some`/`None` é decidido no `set_from_value`.
    if is_option {
        return Ok(());
    }
    let name = match value {
        EcsValue::Nil => return Ok(()),
        EcsValue::Str(name) | EcsValue::Enum(name) => name.as_str(),
        EcsValue::Map(fields) => match fields.iter().find(|(key, _)| key == "variant") {
            Some((_, EcsValue::Str(name) | EcsValue::Enum(name))) => name.as_str(),
            _ => return Ok(()),
        },
        _ => {
            return Err(format!(
                "{path}: enum '{}' espera o nome da variante",
                enum_info.type_path()
            ));
        }
    };
    if enum_info.variant(name).is_none() {
        return Err(format!(
            "{path}: variante '{name}' não existe em {}",
            enum_info.type_path()
        ));
    }
    Ok(())
}

// --------------------------------------------------------------- espelho

/// Reconstrói o espelho do frame e reabastece o pool de spawn.
///
/// Sistema exclusivo: precisa do `&mut World` para `ReflectComponent::register_component`
/// e para a travessia por arquétipo (só arquétipos que contêm o componente —
/// um mundo de 79k entidades não é varrido 16×).
pub fn mirror_system(world: &mut World) {
    let data = build_mirror(world);
    if let Some(mut mirror) = world.get_resource_mut::<EcsMirror>() {
        mirror.0 = data;
    } else {
        world.insert_resource(EcsMirror(data));
    }
    refill_spawn_pool(world);
}

/// Constrói o espelho do frame. Só corre com scripts vivos: sem
/// `LuaScriptRef` ninguém lê o espelho e o editor/menu não paga nada.
pub fn build_mirror(world: &mut World) -> Arc<EcsMirrorData> {
    let any_scripts = {
        let mut query = world.query_filtered::<Entity, With<super::components::LuaScriptRef>>();
        query.iter(world).next().is_some()
    };
    build_mirror_inner(world, any_scripts)
}

/// Constrói o espelho SEM o gate de scripts — a REPL do bridge inspecciona
/// componentes mesmo num mundo parado (editor/menu, antes do 1.º spawn).
pub fn build_mirror_live(world: &mut World) -> Arc<EcsMirrorData> {
    build_mirror_inner(world, true)
}

fn build_mirror_inner(world: &mut World, enabled: bool) -> Arc<EcsMirrorData> {
    let mut data = EcsMirrorData::default();
    if !enabled {
        return Arc::new(data);
    }
    let Ok(registry) = registry_arc(world) else {
        return Arc::new(data);
    };
    let registry = registry.read();
    for type_path in WHITELIST {
        let Some(registration) = registry.get_with_type_path(type_path) else {
            continue;
        };
        let Some(reflection) = registration.data::<ReflectComponent>() else {
            continue;
        };
        // Regista o componente no mundo (id de componente) e percorre só os
        // arquétipos que o contêm.
        let component_id = reflection.register_component(world);
        let mut values = HashMap::new();
        for archetype in world.archetypes().iter() {
            if !archetype.contains(component_id) {
                continue;
            }
            for archetype_entity in archetype.entities() {
                let entity = archetype_entity.id();
                let Some(value) = reflection.reflect(world.entity(entity)) else {
                    continue;
                };
                values.insert(
                    entity.to_bits() as i64,
                    value_from_reflect(value.as_partial_reflect()),
                );
            }
        }
        if !values.is_empty() {
            data.by_type.insert(type_path, values);
        }
    }
    Arc::new(data)
}

/// Reabastece [`EcsSpawnPool`] com entidades vazias (cresce com o uso).
pub fn refill_spawn_pool(world: &mut World) {
    let Some(pool) = world.get_resource::<EcsSpawnPool>() else {
        return;
    };
    let queue = pool.queue.clone();
    let available = queue
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .len();
    let used = pool.filled_last.saturating_sub(available);
    let high_water = pool.high_water.max(used);
    let target = MIN_SPAWN_POOL.max(high_water.saturating_mul(2));
    let want = target.saturating_sub(available);
    if want > 0 {
        let mut ids = Vec::with_capacity(want);
        for _ in 0..want {
            ids.push(world.spawn_empty().id());
        }
        queue
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .extend(ids);
    }
    if let Some(mut pool) = world.get_resource_mut::<EcsSpawnPool>() {
        pool.filled_last = available + want;
        pool.high_water = high_water;
    }
}

// ------------------------------------------------------------- API Luau

/// Instala o grupo `viber.ecs` na tabela `viber`.
pub(crate) fn install(lua: &Lua, api: &Table) -> mlua::Result<()> {
    let ecs = lua.create_table()?;

    // viber.ecs.types() -> {"viber::vitals::Health", …}
    ecs.set(
        "types",
        lua.create_function(|lua, ()| {
            let registered: Option<Vec<&'static str>> =
                lua.app_data_ref::<ScriptCtx>().and_then(|ctx| {
                    ctx.ecs_registry.as_ref().map(|registry| {
                        let registry = registry.read();
                        WHITELIST
                            .iter()
                            .copied()
                            .filter(|path| registry.get_with_type_path(path).is_some())
                            .collect()
                    })
                });
            let mut paths = registered.unwrap_or_else(|| WHITELIST.to_vec());
            paths.sort_unstable();
            let out = lua.create_table()?;
            for (index, path) in paths.into_iter().enumerate() {
                out.raw_set(index + 1, path)?;
            }
            Ok(out)
        })?,
    )?;

    // viber.ecs.get(id, type_path) -> table | nil
    ecs.set(
        "get",
        lua.create_function(|lua, (id, type_path): (i64, String)| {
            ensure_whitelisted(&type_path)?;
            let entity = super::entity::entity_from_id(id)?;
            let bits = entity.to_bits() as i64;
            let ctx = lua.app_data_ref::<ScriptCtx>().ok_or_else(no_vm_ctx)?;
            ensure_registered(&ctx, &type_path)?;
            let mirror = ctx.ecs_mirror.as_ref().ok_or_else(no_mirror)?;
            let Some(value) = mirror.get(&type_path, bits) else {
                return Ok(Value::Nil);
            };
            value_to_lua(lua, value)
        })?,
    )?;

    // viber.ecs.set(id, type_path, tabela) -> ok, erro
    ecs.set(
        "set",
        lua.create_function(|lua, (id, type_path, patch): (i64, String, Table)| {
            ensure_whitelisted(&type_path)?;
            let entity = super::entity::entity_from_id(id)?;
            let bits = entity.to_bits() as i64;
            let patch = value_from_lua(Value::Table(patch))?;
            {
                let ctx = lua.app_data_ref::<ScriptCtx>().ok_or_else(no_vm_ctx)?;
                let registry = ctx.ecs_registry.as_ref().ok_or_else(no_registry)?;
                let registry = registry.read();
                validate_patch(&registry, &type_path, &patch).map_err(mlua::Error::runtime)?;
                let mirror = ctx.ecs_mirror.as_ref().ok_or_else(no_mirror)?;
                if mirror.get(&type_path, bits).is_none() {
                    return Ok((
                        false,
                        Some(format!(
                            "viber.ecs.set: entidade {id} não tem '{type_path}' (usa insert)"
                        )),
                    ));
                }
            }
            push_op(
                lua,
                EcsOp::Set {
                    entity,
                    type_path,
                    patch,
                },
            )?;
            Ok((true, None))
        })?,
    )?;

    // viber.ecs.insert(id, type_path, tabela) -> ok, erro
    ecs.set(
        "insert",
        lua.create_function(|lua, (id, type_path, patch): (i64, String, Table)| {
            ensure_whitelisted(&type_path)?;
            let entity = super::entity::entity_from_id(id)?;
            let patch = value_from_lua(Value::Table(patch))?;
            {
                let ctx = lua.app_data_ref::<ScriptCtx>().ok_or_else(no_vm_ctx)?;
                let registry = ctx.ecs_registry.as_ref().ok_or_else(no_registry)?;
                let registry = registry.read();
                if let Err(error) = validate_patch(&registry, &type_path, &patch) {
                    return Ok((false, Some(format!("viber.ecs.insert: {error}"))));
                }
                // Valida que o tipo é construível AGORA (o comando só
                // corre pós-frame; o erro tem de chegar ao script).
                if let Err(error) = build_component(&registry, &type_path, &patch) {
                    return Ok((false, Some(format!("viber.ecs.insert: {error}"))));
                }
            }
            push_op(
                lua,
                EcsOp::Insert {
                    entity,
                    type_path,
                    patch,
                },
            )?;
            Ok((true, None))
        })?,
    )?;

    // viber.ecs.remove(id, type_path) -> ok, erro
    ecs.set(
        "remove",
        lua.create_function(|lua, (id, type_path): (i64, String)| {
            ensure_whitelisted(&type_path)?;
            let entity = super::entity::entity_from_id(id)?;
            let bits = entity.to_bits() as i64;
            {
                let ctx = lua.app_data_ref::<ScriptCtx>().ok_or_else(no_vm_ctx)?;
                ensure_registered(&ctx, &type_path)?;
                let mirror = ctx.ecs_mirror.as_ref().ok_or_else(no_mirror)?;
                if mirror.get(&type_path, bits).is_none() {
                    return Ok((
                        false,
                        Some(format!(
                            "viber.ecs.remove: entidade {id} não tem '{type_path}'"
                        )),
                    ));
                }
            }
            push_op(lua, EcsOp::Remove { entity, type_path })?;
            Ok((true, None))
        })?,
    )?;

    // viber.ecs.query(type_path) -> {ids}
    ecs.set(
        "query",
        lua.create_function(|lua, type_path: String| {
            ensure_whitelisted(&type_path)?;
            let ctx = lua.app_data_ref::<ScriptCtx>().ok_or_else(no_vm_ctx)?;
            ensure_registered(&ctx, &type_path)?;
            let mirror = ctx.ecs_mirror.as_ref().ok_or_else(no_mirror)?;
            let ids = mirror.ids(&type_path);
            let out = lua.create_table()?;
            for (index, bits) in ids.into_iter().enumerate() {
                out.raw_set(index + 1, bits)?;
            }
            Ok(out)
        })?,
    )?;

    // viber.ecs.spawn({ [type_path] = tabela, … }) -> id
    ecs.set(
        "spawn",
        lua.create_function(|lua, components: Table| {
            let mut entries: Vec<(String, EcsValue)> = Vec::new();
            for pair in components.pairs::<String, Value>() {
                let (type_path, value) = pair?;
                ensure_whitelisted(&type_path)?;
                let patch = value_from_lua(value)?;
                entries.push((type_path, patch));
            }
            if entries.is_empty() {
                return Err(mlua::Error::runtime(
                    "viber.ecs.spawn: a tabela de componentes está vazia",
                ));
            }
            // Ordem determinística: a iteração de `pairs` não é.
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            {
                let ctx = lua.app_data_ref::<ScriptCtx>().ok_or_else(no_vm_ctx)?;
                let registry = ctx.ecs_registry.as_ref().ok_or_else(no_registry)?;
                let registry = registry.read();
                for (type_path, patch) in &entries {
                    validate_patch(&registry, type_path, patch).map_err(mlua::Error::runtime)?;
                    build_component(&registry, type_path, patch).map_err(mlua::Error::runtime)?;
                }
            }
            let pool = lua
                .app_data_ref::<ScriptCtx>()
                .and_then(|ctx| ctx.ecs_pool.clone())
                .ok_or_else(|| {
                    mlua::Error::runtime(
                        "viber.ecs.spawn indisponível: o LuauScriptPlugin não instalou o pool",
                    )
                })?;
            let entity = pool
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .pop()
                .ok_or_else(|| {
                    mlua::Error::runtime(
                        "viber.ecs.spawn: orçamento de spawns deste frame esgotado — \
                         tenta no frame seguinte",
                    )
                })?;
            push_op(
                lua,
                EcsOp::Spawn {
                    entity,
                    components: entries,
                },
            )?;
            Ok(entity.to_bits() as i64)
        })?,
    )?;

    // viber.ecs.despawn(id)
    ecs.set(
        "despawn",
        lua.create_function(|lua, id: i64| {
            let entity = super::entity::entity_from_id(id)?;
            push_op(lua, EcsOp::Despawn { entity })?;
            Ok(())
        })?,
    )?;

    api.set("ecs", ecs)?;
    Ok(())
}

/// Exige o `AppTypeRegistry` e que `type_path` esteja registado (além da
/// whitelist estática): um tipo que o `ReflectPlugin` não registou não pode
/// ser lido nem escrito.
fn ensure_registered(ctx: &ScriptCtx, type_path: &str) -> mlua::Result<()> {
    let registry = ctx.ecs_registry.as_ref().ok_or_else(no_registry)?;
    if registry.read().get_with_type_path(type_path).is_none() {
        return Err(mlua::Error::runtime(format!(
            "viber.ecs: '{type_path}' não está no AppTypeRegistry deste mundo"
        )));
    }
    Ok(())
}

fn ensure_whitelisted(type_path: &str) -> mlua::Result<()> {
    if WHITELIST.contains(&type_path) {
        Ok(())
    } else {
        Err(mlua::Error::runtime(format!(
            "viber.ecs: tipo '{type_path}' não está na whitelist (vê viber.ecs.types())"
        )))
    }
}

fn push_op(lua: &Lua, op: EcsOp) -> mlua::Result<()> {
    lua.app_data_mut::<ScriptCtx>()
        .expect("ScriptCtx app data seeded in LuaScriptHost::new")
        .commands
        .push(ScriptCommand::Ecs(op));
    Ok(())
}

fn no_vm_ctx() -> mlua::Error {
    mlua::Error::runtime("viber.ecs indisponível fora do runtime Luau (sem ScriptCtx)")
}

fn no_mirror() -> mlua::Error {
    mlua::Error::runtime(
        "viber.ecs indisponível: sem espelho do mundo neste frame \
         (o LuauScriptPlugin não está instalado?)",
    )
}

fn no_registry() -> mlua::Error {
    mlua::Error::runtime(
        "viber.ecs indisponível: a VM não recebeu o AppTypeRegistry \
         (ReflectPlugin/LuauScriptPlugin não instalados)",
    )
}

// --------------------------------------------------------------- testes

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reflect::ReflectPlugin;
    use crate::vitals::Health;

    fn test_world() -> App {
        let mut app = App::new();
        app.add_plugins(ReflectPlugin);
        // O espelho só corre com scripts vivos.
        app.world_mut()
            .spawn(super::super::components::LuaScriptRef {
                path: "test.lua".into(),
            });
        app
    }

    #[test]
    fn paths_da_engine_batem_certo() {
        assert_eq!(
            <Transform as bevy::reflect::TypePath>::type_path(),
            TRANSFORM_PATH
        );
        assert_eq!(
            <Visibility as bevy::reflect::TypePath>::type_path(),
            VISIBILITY_PATH
        );
    }

    #[test]
    fn whitelist_toda_registada_e_componente() {
        let mut app = App::new();
        app.add_plugins(ReflectPlugin);
        let registry = app
            .world()
            .resource::<bevy::ecs::reflect::AppTypeRegistry>();
        let registry = registry.read();
        for path in WHITELIST {
            let registration = registry
                .get_with_type_path(path)
                .unwrap_or_else(|| panic!("{path} não registado"));
            assert!(
                registration.data::<ReflectComponent>().is_some(),
                "{path} sem ReflectComponent"
            );
        }
    }

    #[test]
    fn mirror_le_e_patch_altera_o_componente() {
        let mut app = test_world();
        let entity = app
            .world_mut()
            .spawn((
                Health {
                    current: 30.0,
                    max: 80.0,
                },
                Transform::default(),
            ))
            .id();
        let bits = entity.to_bits() as i64;

        let mirror = build_mirror(app.world_mut());
        match mirror.get("viber::vitals::Health", bits) {
            Some(EcsValue::Map(fields)) => {
                assert!(
                    fields.iter().any(|(name, value)| {
                        name == "current" && *value == EcsValue::Float(30.0)
                    })
                );
                assert!(
                    fields
                        .iter()
                        .any(|(name, value)| { name == "max" && *value == EcsValue::Float(80.0) })
                );
            }
            other => panic!("Health refletido: {other:?}"),
        }
        assert_eq!(mirror.ids("viber::vitals::Health"), vec![bits]);

        let patch = EcsValue::Map(vec![("current".into(), EcsValue::Float(12.0))]);
        assert_eq!(
            patch_component(app.world_mut(), entity, "viber::vitals::Health", &patch),
            Ok(true)
        );
        let health = app.world().get::<Health>(entity).unwrap();
        assert!((health.current - 12.0).abs() < 1e-6);
        assert!((health.max - 80.0).abs() < 1e-6);

        // `set_pose`/`set_scale` internos: Transform não está na whitelist.
        let pose = EcsValue::Map(vec![
            ("translation".into(), EcsValue::Vec3([1.0, 2.0, 3.0])),
            (
                "rotation".into(),
                EcsValue::Quat([0.0, (0.5f32).sin(), 0.0, (0.5f32).cos()]),
            ),
        ]);
        assert_eq!(
            patch_component(app.world_mut(), entity, TRANSFORM_PATH, &pose),
            Ok(true)
        );
        let transform = app.world().get::<Transform>(entity).unwrap();
        assert_eq!(transform.translation, Vec3::new(1.0, 2.0, 3.0));
        assert!((transform.rotation.y - (0.5f32).sin()).abs() < 1e-6);
    }

    #[test]
    fn insert_e_remove_respeitam_a_construcao() {
        let mut app = test_world();
        let entity = app.world_mut().spawn(Transform::default()).id();
        let patch = EcsValue::Map(vec![
            ("current".into(), EcsValue::Float(5.0)),
            ("max".into(), EcsValue::Float(9.0)),
        ]);
        insert_component(app.world_mut(), entity, "viber::vitals::Health", &patch).unwrap();
        let health = app.world().get::<Health>(entity).unwrap();
        assert_eq!((health.current, health.max), (5.0, 9.0));

        // Unitário: `Hostile` sem campos (Default do próprio tipo).
        insert_component(
            app.world_mut(),
            entity,
            "viber::vitals::Hostile",
            &EcsValue::Unit,
        )
        .unwrap();
        assert!(app.world().get::<crate::vitals::Hostile>(entity).is_some());

        // Campo inexistente: erro claro, sem tocar no componente.
        let bad = EcsValue::Map(vec![("nope".into(), EcsValue::Float(1.0))]);
        assert!(
            patch_component(app.world_mut(), entity, "viber::vitals::Health", &bad)
                .unwrap_err()
                .contains("campo 'nope'")
        );

        assert_eq!(
            remove_component(app.world_mut(), entity, "viber::vitals::Health"),
            Ok(true)
        );
        assert!(app.world().get::<Health>(entity).is_none());
    }

    #[test]
    fn insert_sem_default_constroi_todos_os_campos() {
        let mut app = test_world();
        let entity = app.world_mut().spawn(Transform::default()).id();
        let patch = EcsValue::Map(vec![
            (
                "vel".into(),
                EcsValue::Map(vec![
                    ("x".into(), EcsValue::Float(1.0)),
                    ("y".into(), EcsValue::Float(2.0)),
                    ("z".into(), EcsValue::Float(3.0)),
                ]),
            ),
            ("life".into(), EcsValue::Float(2.0)),
            ("damage".into(), EcsValue::Float(5.0)),
            ("sensor_radius".into(), EcsValue::Float(0.1)),
            ("faction".into(), EcsValue::Str("Enemy".into())),
            ("gravity".into(), EcsValue::Float(9.8)),
            ("shooter".into(), EcsValue::Entity(entity.to_bits() as i64)),
        ]);
        insert_component(
            app.world_mut(),
            entity,
            "viber::projectile::Projectile",
            &patch,
        )
        .expect("insert sem Default");
        let projectile = app
            .world()
            .get::<crate::projectile::Projectile>(entity)
            .unwrap();
        assert_eq!(projectile.vel, Vec3::new(1.0, 2.0, 3.0));
        assert_eq!(projectile.faction, crate::projectile::Faction::Enemy);
        assert_eq!(projectile.shooter, Some(entity));

        // Sem um campo e sem Default: erro claro (não constrói a zeros).
        let partial = EcsValue::Map(vec![("vel".into(), EcsValue::Vec3([0.0, 0.0, 0.0]))]);
        let error = insert_component(
            app.world_mut(),
            entity,
            "viber::projectile::Projectile",
            &partial,
        )
        .unwrap_err();
        assert!(error.contains("faltam campos"), "{error}");
    }

    #[test]
    fn patch_option_e_enum_por_nome() {
        let mut app = test_world();
        let entity = app
            .world_mut()
            .spawn((crate::ai::EnemyCreature::default(), Transform::default()))
            .id();
        let patch = EcsValue::Map(vec![
            (
                "home".into(),
                EcsValue::Map(vec![
                    ("x".into(), EcsValue::Float(3.0)),
                    ("y".into(), EcsValue::Float(-4.0)),
                ]),
            ),
            ("state".into(), EcsValue::Str("Chase".into())),
            ("aggro_radius".into(), EcsValue::Float(11.0)),
        ]);
        assert_eq!(
            patch_component(app.world_mut(), entity, "viber::ai::EnemyCreature", &patch),
            Ok(true)
        );
        let creature = app.world().get::<crate::ai::EnemyCreature>(entity).unwrap();
        assert_eq!(creature.home, Some(Vec2::new(3.0, -4.0)));
        assert_eq!(creature.state, crate::ai::EnemyState::Chase);
        assert_eq!(creature.aggro_radius, 11.0);

        // `Option` de volta a `None` por "none".
        let patch = EcsValue::Map(vec![("home".into(), EcsValue::Str("none".into()))]);
        patch_component(app.world_mut(), entity, "viber::ai::EnemyCreature", &patch).unwrap();
        assert_eq!(
            app.world()
                .get::<crate::ai::EnemyCreature>(entity)
                .unwrap()
                .home,
            None
        );
    }

    #[test]
    fn spawn_pool_cresce_e_ops_aplicam() {
        let mut app = test_world();
        app.world_mut().insert_resource(EcsSpawnPool::default());
        refill_spawn_pool(app.world_mut());
        let entity = app
            .world()
            .resource::<EcsSpawnPool>()
            .take()
            .expect("id do pool");
        assert!(app.world().get_entity(entity).is_ok());

        let op = EcsOp::Spawn {
            entity,
            components: vec![(
                "viber::vitals::Health".into(),
                EcsValue::Map(vec![
                    ("current".into(), EcsValue::Float(7.0)),
                    ("max".into(), EcsValue::Float(11.0)),
                ]),
            )],
        };
        apply_op(app.world_mut(), &op).unwrap();
        assert_eq!(app.world().get::<Health>(entity).unwrap().current, 7.0);
    }
}
