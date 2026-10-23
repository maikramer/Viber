//! `viber.decision` — d1-omni como serviço de decisão.
//!
//! Mesmo desenho do `viber.nlu`: o modelo carrega numa worker thread e o jogo
//! fala por canais (`submit` devolve id, `poll` drena). Uma decisão é um
//! `state` + um mapa de perguntas tipadas; a resposta é `{answers={...}}`.
//!
//! ```lua
//! viber.decision.load("assets/public/d1")
//! local id = viber.decision.submit(state, {
//!   intent = { type="choice", instructions="O que quer o jogador fazer?",
//!              criteria={ {key="comprar", description="comprar um item"}, ... } },
//!   ameaca = { type="noul", instructions="O jogador está a ameaçar?" },
//! })
//! local r = viber.decision.poll()   -- {kind="decided", answers={...}}
//! ```
//!
//! Sem a feature `decision` (ou sem o artefacto), `available()` é `false` e os
//! scripts caem no fluxo antigo.

#[cfg(feature = "decision")]
mod imp {
    use std::collections::VecDeque;
    use std::path::{Component, Path, PathBuf};
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use mlua::{Lua, Table};
    use serde_json::{Map, Value};

    use super::super::ctx::ScriptCtx;

    const ARTIFACT_FILES: &[&str] = &[
        "model.onnx",
        "model.onnx.data",
        "embedding.bin",
        "tokenizer.json",
        "decision_config.json",
    ];

    enum Request {
        Load(PathBuf),
        Decide {
            id: u32,
            state: Value,
            questions: Map<String, Value>,
        },
    }

    enum Response {
        Loaded(Result<(), String>),
        Decided {
            id: u32,
            result: Result<String, String>,
        },
    }

    #[derive(Clone)]
    struct DecisionState {
        tx: Sender<Request>,
        rx: Arc<Mutex<Receiver<Response>>>,
        pending: Arc<Mutex<VecDeque<Response>>>,
        next_id: Arc<AtomicU32>,
        ready: Arc<AtomicBool>,
        load_error: Arc<Mutex<Option<String>>>,
    }

    fn worker(
        rx: Receiver<Request>,
        tx: Sender<Response>,
        ready: Arc<AtomicBool>,
        load_error: Arc<Mutex<Option<String>>>,
    ) {
        let mut runtime: Option<vael_decision::Runtime> = None;
        while let Ok(req) = rx.recv() {
            match req {
                Request::Load(dir) => {
                    if runtime.is_some() {
                        let _ = tx.send(Response::Loaded(Ok(())));
                        continue;
                    }
                    let loaded = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        vael_decision::Runtime::load(&dir)
                    }));
                    match loaded {
                        Ok(Ok(rt)) => {
                            runtime = Some(rt);
                            *load_error.lock().unwrap() = None;
                            ready.store(true, Ordering::Release);
                            let _ = tx.send(Response::Loaded(Ok(())));
                        }
                        Ok(Err(e)) => {
                            let msg = format!("{e:#}");
                            *load_error.lock().unwrap() = Some(msg.clone());
                            let _ = tx.send(Response::Loaded(Err(msg)));
                        }
                        Err(_) => {
                            let msg = "viber.decision: panic ao carregar o modelo".to_string();
                            *load_error.lock().unwrap() = Some(msg.clone());
                            let _ = tx.send(Response::Loaded(Err(msg)));
                        }
                    }
                }
                Request::Decide {
                    id,
                    state,
                    questions,
                } => {
                    let result = match runtime.as_mut() {
                        Some(rt) => {
                            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                rt.decide(&state, &questions)
                            })) {
                                Ok(Ok(v)) => Ok(v.to_string()),
                                Ok(Err(e)) => Err(format!("{e:#}")),
                                Err(_) => Err("viber.decision: panic na inferência".to_string()),
                            }
                        }
                        None => Err("viber.decision: modelo não carregado".to_string()),
                    };
                    let _ = tx.send(Response::Decided { id, result });
                }
            }
        }
    }

    fn resolve_dir(lua: &Lua, rel: &str) -> mlua::Result<PathBuf> {
        let relative = Path::new(rel);
        let inside = relative
            .components()
            .all(|c| matches!(c, Component::Normal(_) | Component::CurDir));
        if rel.is_empty() || !inside {
            return Err(mlua::Error::runtime(format!(
                "viber.decision.load('{rel}'): só caminhos relativos à pasta do jogo (sem '..')"
            )));
        }
        let scripts = lua
            .app_data_ref::<ScriptCtx>()
            .and_then(|ctx| ctx.scripts_dir.clone())
            .ok_or_else(|| mlua::Error::runtime("viber.decision.load sem scripts_dir"))?;
        let world = scripts.parent().ok_or_else(|| {
            mlua::Error::runtime("viber.decision.load: scripts/ sem pasta de jogo")
        })?;
        Ok(world.join(relative))
    }

    fn files_present(dir: &Path) -> bool {
        ARTIFACT_FILES.iter().all(|f| dir.join(f).is_file())
    }

    fn state(lua: &Lua) -> mlua::Result<DecisionState> {
        lua.app_data_ref::<DecisionState>()
            .map(|st| st.clone())
            .ok_or_else(|| mlua::Error::runtime("viber.decision: estado ausente"))
    }

    fn lua_to_json(v: &mlua::Value) -> mlua::Result<Value> {
        Ok(match v {
            mlua::Value::Nil => Value::Null,
            mlua::Value::Boolean(b) => Value::Bool(*b),
            mlua::Value::Integer(i) => Value::from(*i),
            mlua::Value::Number(n) => Value::from(*n),
            mlua::Value::String(s) => Value::String(s.to_str()?.to_string()),
            mlua::Value::Table(t) => {
                let len = t.raw_len();
                let mut is_array = len > 0;
                if is_array {
                    for pair in t.clone().pairs::<mlua::Value, mlua::Value>() {
                        let (k, _) = pair?;
                        match k {
                            mlua::Value::Integer(i) if i >= 1 && i <= len as i64 => {}
                            _ => {
                                is_array = false;
                                break;
                            }
                        }
                    }
                }
                if is_array {
                    let mut a = Vec::with_capacity(len);
                    for i in 1..=len {
                        a.push(lua_to_json(&t.get::<mlua::Value>(i)?)?);
                    }
                    Value::Array(a)
                } else {
                    let mut o = Map::new();
                    for pair in t.clone().pairs::<mlua::Value, mlua::Value>() {
                        let (k, val) = pair?;
                        let key = match k {
                            mlua::Value::String(s) => s.to_str()?.to_string(),
                            mlua::Value::Integer(i) => i.to_string(),
                            _ => {
                                return Err(mlua::Error::runtime(
                                    "viber.decision: chave de tabela não é string/inteiro",
                                ));
                            }
                        };
                        o.insert(key, lua_to_json(&val)?);
                    }
                    Value::Object(o)
                }
            }
            other => {
                return Err(mlua::Error::runtime(format!(
                    "viber.decision: valor não convertível para JSON: {}",
                    other.type_name()
                )));
            }
        })
    }

    fn json_to_lua(lua: &Lua, v: &Value) -> mlua::Result<mlua::Value> {
        Ok(match v {
            Value::Null => mlua::Value::Nil,
            Value::Bool(b) => mlua::Value::Boolean(*b),
            Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    mlua::Value::Integer(i)
                } else {
                    mlua::Value::Number(n.as_f64().unwrap_or(0.0))
                }
            }
            Value::String(s) => mlua::Value::String(lua.create_string(s)?),
            Value::Array(a) => {
                let t = lua.create_table()?;
                for (i, x) in a.iter().enumerate() {
                    t.set(i + 1, json_to_lua(lua, x)?)?;
                }
                mlua::Value::Table(t)
            }
            Value::Object(o) => {
                let t = lua.create_table()?;
                for (k, x) in o {
                    t.set(k.as_str(), json_to_lua(lua, x)?)?;
                }
                mlua::Value::Table(t)
            }
        })
    }

    fn response_to_value(lua: &Lua, resp: Response) -> mlua::Result<mlua::Value> {
        match resp {
            Response::Loaded(Ok(())) => {
                let t = lua.create_table()?;
                t.set("kind", "loaded")?;
                t.set("ok", true)?;
                Ok(mlua::Value::Table(t))
            }
            Response::Loaded(Err(e)) => {
                let t = lua.create_table()?;
                t.set("kind", "loaded")?;
                t.set("ok", false)?;
                t.set("error", e)?;
                Ok(mlua::Value::Table(t))
            }
            Response::Decided { id, result: Ok(s) } => {
                let v: Value = serde_json::from_str(&s)
                    .map_err(|e| mlua::Error::runtime(format!("viber.decision: JSON: {e}")))?;
                let t = match json_to_lua(lua, &v)? {
                    mlua::Value::Table(t) => t,
                    _ => lua.create_table()?,
                };
                t.set("kind", "decided")?;
                t.set("id", id)?;
                Ok(mlua::Value::Table(t))
            }
            Response::Decided { id, result: Err(e) } => {
                let t = lua.create_table()?;
                t.set("kind", "error")?;
                t.set("id", id)?;
                t.set("error", e)?;
                Ok(mlua::Value::Table(t))
            }
        }
    }

    fn payloads(
        state_v: mlua::Value,
        questions_v: mlua::Value,
    ) -> mlua::Result<(Value, Map<String, Value>)> {
        let state = lua_to_json(&state_v)?;
        let questions = match lua_to_json(&questions_v)? {
            Value::Object(o) => o,
            _ => {
                return Err(mlua::Error::runtime(
                    "viber.decision: questions tem de ser uma tabela nomeada",
                ));
            }
        };
        Ok((state, questions))
    }

    pub(crate) fn install(lua: &Lua, api: &Table) -> mlua::Result<()> {
        let (req_tx, req_rx) = mpsc::channel();
        let (res_tx, res_rx) = mpsc::channel();
        let ready = Arc::new(AtomicBool::new(false));
        let load_error = Arc::new(Mutex::new(None::<String>));

        let (w_ready, w_err) = (ready.clone(), load_error.clone());
        std::thread::Builder::new()
            .name("vael-decision".into())
            .spawn(move || worker(req_rx, res_tx, w_ready, w_err))
            .map_err(|e| mlua::Error::runtime(format!("viber.decision: worker: {e}")))?;

        lua.set_app_data(DecisionState {
            tx: req_tx,
            rx: Arc::new(Mutex::new(res_rx)),
            pending: Arc::new(Mutex::new(VecDeque::new())),
            next_id: Arc::new(AtomicU32::new(1)),
            ready,
            load_error,
        });

        let d = lua.create_table()?;

        d.set(
            "available",
            lua.create_function(|lua, dir: Option<String>| {
                let Some(dir) = dir else {
                    let st = state(lua)?;
                    return Ok(st.ready.load(Ordering::Acquire));
                };
                let path = resolve_dir(lua, &dir)?;
                Ok(files_present(&path))
            })?,
        )?;

        d.set(
            "last_error",
            lua.create_function(|lua, ()| {
                let st = state(lua)?;
                let e = st.load_error.lock().unwrap().clone();
                Ok(match e {
                    Some(s) => mlua::Value::String(lua.create_string(s)?),
                    None => mlua::Value::Nil,
                })
            })?,
        )?;

        d.set(
            "ready",
            lua.create_function(|lua, ()| {
                let st = state(lua)?;
                Ok(st.ready.load(Ordering::Acquire))
            })?,
        )?;

        d.set(
            "load",
            lua.create_function(|lua, dir: String| {
                let path = resolve_dir(lua, &dir)?;
                let st = state(lua)?;
                if st.ready.load(Ordering::Acquire) {
                    return Ok((true, mlua::Value::Nil));
                }
                if !files_present(&path) {
                    return Ok((
                        false,
                        mlua::Value::String(lua.create_string(format!(
                            "viber.decision: artefacto incompleto em {}",
                            path.display()
                        ))?),
                    ));
                }
                st.tx
                    .send(Request::Load(path))
                    .map_err(|_| mlua::Error::runtime("viber.decision: worker parado"))?;
                Ok((true, mlua::Value::Nil))
            })?,
        )?;

        d.set(
            "submit",
            lua.create_function(|lua, (state_v, questions_v): (mlua::Value, mlua::Value)| {
                let st = state(lua)?;
                if !st.ready.load(Ordering::Acquire) {
                    return Ok((
                        mlua::Value::Nil,
                        mlua::Value::String(
                            lua.create_string("viber.decision: modelo ainda não carregado")?,
                        ),
                    ));
                }
                let (state, questions) = payloads(state_v, questions_v)?;
                let id = st.next_id.fetch_add(1, Ordering::Relaxed);
                st.tx
                    .send(Request::Decide {
                        id,
                        state,
                        questions,
                    })
                    .map_err(|_| mlua::Error::runtime("viber.decision: worker parado"))?;
                Ok((mlua::Value::Integer(id as i64), mlua::Value::Nil))
            })?,
        )?;

        d.set(
            "poll",
            lua.create_function(|lua, ()| {
                let st = state(lua)?;
                let queued = {
                    let mut pending = st.pending.lock().unwrap();
                    pending.pop_front()
                };
                if let Some(resp) = queued {
                    return response_to_value(lua, resp);
                }
                let rx = st.rx.lock().unwrap();
                match rx.try_recv() {
                    Ok(resp) => response_to_value(lua, resp),
                    Err(TryRecvError::Empty) => Ok(mlua::Value::Nil),
                    Err(TryRecvError::Disconnected) => {
                        Err(mlua::Error::runtime("viber.decision: worker terminou"))
                    }
                }
            })?,
        )?;

        // decide(state, questions) -> table, err — bloqueante; QA/ferramentas.
        d.set(
            "decide",
            lua.create_function(|lua, (state_v, questions_v): (mlua::Value, mlua::Value)| {
                let st = state(lua)?;
                if !st.ready.load(Ordering::Acquire) {
                    return Ok((
                        mlua::Value::Nil,
                        mlua::Value::String(
                            lua.create_string("viber.decision: modelo ainda não carregado")?,
                        ),
                    ));
                }
                let (state, questions) = payloads(state_v, questions_v)?;
                let id = st.next_id.fetch_add(1, Ordering::Relaxed);
                st.tx
                    .send(Request::Decide {
                        id,
                        state,
                        questions,
                    })
                    .map_err(|_| mlua::Error::runtime("viber.decision: worker parado"))?;
                let rx = st.rx.lock().unwrap();
                let deadline = Instant::now() + Duration::from_secs(30);
                loop {
                    let left = deadline.saturating_duration_since(Instant::now());
                    if left.is_zero() {
                        return Ok((
                            mlua::Value::Nil,
                            mlua::Value::String(
                                lua.create_string("viber.decision.decide: timeout")?,
                            ),
                        ));
                    }
                    match rx.recv_timeout(left) {
                        Ok(Response::Decided { id: rid, result }) if rid == id => {
                            return match result {
                                Ok(s) => {
                                    let v: Value = serde_json::from_str(&s).map_err(|e| {
                                        mlua::Error::runtime(format!("viber.decision: JSON: {e}"))
                                    })?;
                                    Ok((json_to_lua(lua, &v)?, mlua::Value::Nil))
                                }
                                Err(e) => Ok((
                                    mlua::Value::Nil,
                                    mlua::Value::String(lua.create_string(e)?),
                                )),
                            };
                        }
                        Ok(resp) => {
                            st.pending.lock().unwrap().push_back(resp);
                            continue;
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => {
                            return Ok((
                                mlua::Value::Nil,
                                mlua::Value::String(
                                    lua.create_string("viber.decision.decide: timeout")?,
                                ),
                            ));
                        }
                        Err(mpsc::RecvTimeoutError::Disconnected) => {
                            return Err(mlua::Error::runtime("viber.decision: worker terminou"));
                        }
                    }
                }
            })?,
        )?;

        api.set("decision", d)?;
        Ok(())
    }
}

#[cfg(not(feature = "decision"))]
mod imp {
    use mlua::{Lua, Table};

    pub(crate) fn install(_lua: &Lua, _api: &Table) -> mlua::Result<()> {
        Ok(())
    }
}

pub(crate) use imp::install;

#[cfg(all(test, feature = "decision"))]
mod tests {
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    use mlua::Lua;

    use crate::luau::ctx::ScriptCtx;

    #[test]
    fn lua_api_card_example() {
        let viber = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let vael = viber.parent().expect("checkout irmão").join("Vael");
        if !vael.join("assets/public/d1/model.onnx").is_file() {
            eprintln!("skip: artefacto ausente em {}", vael.display());
            return;
        }
        let lua = Lua::new();
        lua.set_app_data(ScriptCtx {
            scripts_dir: Some(vael.join("scripts")),
            ..Default::default()
        });
        let api = lua.create_table().unwrap();
        super::install(&lua, &api).unwrap();
        let d: mlua::Table = api.get("decision").unwrap();

        let available: mlua::Function = d.get("available").unwrap();
        assert!(available.call::<bool>("assets/public/d1").unwrap());

        let load: mlua::Function = d.get("load").unwrap();
        let (ok, err): (bool, mlua::Value) = load.call("assets/public/d1").unwrap();
        assert!(ok, "load: {err:?}");

        let poll: mlua::Function = d.get("poll").unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if let mlua::Value::Table(t) = poll.call::<mlua::Value>(()).unwrap() {
                let kind: String = t.get("kind").unwrap();
                if kind == "loaded" {
                    assert!(t.get::<bool>("ok").unwrap(), "load falhou");
                    break;
                }
                if kind == "error" {
                    let e: String = t.get("error").unwrap();
                    panic!("poll: {e}");
                }
            }
            assert!(Instant::now() < deadline, "timeout no load");
            std::thread::sleep(Duration::from_millis(20));
        }

        let q = lua.create_table().unwrap();
        q.set("type", "choice").unwrap();
        q.set("instructions", "Which team should handle this?")
            .unwrap();
        let crit = lua.create_table().unwrap();
        for (i, (key, desc)) in [
            ("billing", "payments and refunds"),
            ("shipping", ""),
            ("technical", ""),
        ]
        .iter()
        .enumerate()
        {
            let o = lua.create_table().unwrap();
            o.set("key", *key).unwrap();
            if !desc.is_empty() {
                o.set("description", *desc).unwrap();
            }
            crit.set(i + 1, o).unwrap();
        }
        q.set("criteria", crit).unwrap();
        let questions = lua.create_table().unwrap();
        questions.set("route", q).unwrap();

        let decide: mlua::Function = d.get("decide").unwrap();
        let (res, err): (mlua::Value, mlua::Value) = decide
            .call((
                "I was charged twice for my order last week and nobody has replied.",
                questions,
            ))
            .unwrap();
        assert!(err.is_nil(), "decide: {err:?}");
        let t = res.as_table().unwrap();
        let answers: mlua::Table = t.get("answers").unwrap();
        let route: mlua::Table = answers.get("route").unwrap();
        assert_eq!(route.get::<String>("choice").unwrap(), "billing");
    }

    #[test]
    fn lua_api_intent_pt_multi_question() {
        let viber = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let vael = viber.parent().expect("checkout irmão").join("Vael");
        if !vael.join("assets/public/d1/model.onnx").is_file() {
            eprintln!("skip: artefacto ausente em {}", vael.display());
            return;
        }
        let lua = Lua::new();
        lua.set_app_data(ScriptCtx {
            scripts_dir: Some(vael.join("scripts")),
            ..Default::default()
        });
        let api = lua.create_table().unwrap();
        super::install(&lua, &api).unwrap();
        let d: mlua::Table = api.get("decision").unwrap();
        let load: mlua::Function = d.get("load").unwrap();
        let (ok, _): (bool, mlua::Value) = load.call("assets/public/d1").unwrap();
        assert!(ok);
        let poll: mlua::Function = d.get("poll").unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if let mlua::Value::Table(t) = poll.call::<mlua::Value>(()).unwrap() {
                if t.get::<String>("kind").unwrap() == "loaded" {
                    break;
                }
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(20));
        }

        let state = lua.create_table().unwrap();
        state.set("utterance", "vou matar-te").unwrap();
        state.set("npc", "Greta, mercadora").unwrap();

        let intent = lua.create_table().unwrap();
        intent.set("type", "choice").unwrap();
        intent
            .set("instructions", "O que quer o jogador fazer?")
            .unwrap();
        let crit = lua.create_table().unwrap();
        for (i, (k, v)) in [
            ("comprar", "comprar um item"),
            ("falar", "conversar"),
            ("atacar", "atacar ou ameaçar"),
            ("fugir", "afastar-se do perigo"),
            ("descansar", "recuperar vida"),
        ]
        .iter()
        .enumerate()
        {
            let o = lua.create_table().unwrap();
            o.set("key", *k).unwrap();
            o.set("description", *v).unwrap();
            crit.set(i + 1, o).unwrap();
        }
        intent.set("criteria", crit).unwrap();

        let threat = lua.create_table().unwrap();
        threat.set("type", "noul").unwrap();
        threat
            .set("instructions", "O jogador está a ameaçar o mercador?")
            .unwrap();
        let nc = lua.create_table().unwrap();
        nc.set("false", "não é ameaça").unwrap();
        nc.set("true", "é ameaça").unwrap();
        threat.set("criteria", nc).unwrap();

        let questions = lua.create_table().unwrap();
        questions.set("intent", intent).unwrap();
        questions.set("ameaca", threat).unwrap();
        let decide: mlua::Function = d.get("decide").unwrap();
        let (res, err): (mlua::Value, mlua::Value) = decide.call((state, questions)).unwrap();
        assert!(err.is_nil(), "decide: {err:?}");
        let answers: mlua::Table = res.as_table().unwrap().get("answers").unwrap();
        let i: mlua::Table = answers.get("intent").unwrap();
        assert_eq!(i.get::<String>("choice").unwrap(), "atacar");
        let a: mlua::Table = answers.get("ameaca").unwrap();
        let p: f64 = a.get("noul").unwrap();
        assert!((0.0..=1.0).contains(&p), "noul fora de [0,1]: {p}");
    }
}
