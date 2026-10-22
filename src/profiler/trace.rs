//! Gravador de traces do profiler — o que aconteceu, frame a frame, com
//! tempo por sistema e marcadores.
//!
//! O `viber debug prof` responde "como está o frame"; um trace responde "o que
//! tornou ESTE frame lento". O gravador vive em globais (os sistemas correm em
//! paralelo e o wrapper [`super::timed::timed`] não tem acesso ao `World`) e
//! só faz trabalho quando [`start`] foi chamado — fora de uma gravação o custo
//! é um `AtomicBool`.
//!
//! Dois formatos de exportação:
//! * **chrome** — Trace Event Format (Perfetto, `chrome://tracing`): spans por
//!   sistema/script/âncora, contadores por frame e marcas;
//! * **json** — o mesmo conteúdo em JSON nativo do Viber, compacto (CI,
//!   comparações por script).
//!
//! O trace grava os tempos que o profiler já recolhe (o `run_unsafe` de cada
//! sistema instrumentado + as âncoras de schedule/render). A duração é real; o
//! instante de fim vem do `Instant::now()` do registo, por isso sistemas em
//! paralelo aparecem sobrepostos em vez de serializados.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use serde_json::{Value, json};

use super::Group;

/// Teto de eventos por omissão (~1 M spans ≈ poucos MB em disco).
pub const DEFAULT_MAX_EVENTS: usize = 1_000_000;
/// Teto absoluto de eventos (um `--max-events` absurdo não pode esgotar a RAM).
const MAX_EVENTS_CEILING: usize = 10_000_000;
/// Teto de frames: ~1 h a 60 fps. Ultrapassar fecha o trace como truncado.
const MAX_FRAMES: usize = 216_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EventKind {
    System,
    Script,
    Mark,
}

impl EventKind {
    fn as_str(self) -> &'static str {
        match self {
            EventKind::System => "system",
            EventKind::Script => "script",
            EventKind::Mark => "mark",
        }
    }

    /// Thread virtual no Chrome trace: 1 main, 2 scripts, 3 frame/counters.
    fn tid(self) -> u32 {
        match self {
            EventKind::System | EventKind::Mark => 1,
            EventKind::Script => 2,
        }
    }
}

struct TraceEvent {
    frame: u64,
    /// Início do span, em µs desde o arranque da gravação (já descontada a
    /// duração).
    ts_us: u64,
    dur_us: u64,
    name: Cow<'static, str>,
    group: &'static str,
    kind: EventKind,
}

struct TraceFrame {
    frame: u64,
    ts_us: u64,
    ms: f32,
    hitch: bool,
    /// Topo do frame quando é um hitch (atribuição pronta no resumo/CLI).
    hitch_top: Option<Value>,
    counters: Option<Value>,
}

struct Recorder {
    start: Instant,
    started_at: f64,
    max_seconds: f32,
    max_events: usize,
    events: Vec<TraceEvent>,
    frames: Vec<TraceFrame>,
    dropped: u64,
    truncated: bool,
}

impl Recorder {
    fn elapsed_s(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }

    fn ts_us(&self) -> u64 {
        self.start.elapsed().as_micros().min(u64::MAX as u128) as u64
    }
}

struct Finished {
    started_at: f64,
    duration_s: f64,
    max_seconds: f32,
    max_events: usize,
    events: Vec<TraceEvent>,
    frames: Vec<TraceFrame>,
    dropped: u64,
    truncated: bool,
    /// Análise pronta (top sistemas, percentis, hitches) — calculada UMA vez
    /// no `finish`, para o `trace_status` não varrer milhares de eventos a
    /// cada publicação do profiler.
    summary: Value,
}

impl Finished {
    fn summary(&self) -> Value {
        self.summary.clone()
    }
}

struct State {
    recording: Option<Recorder>,
    last: Option<Finished>,
}

impl State {
    const fn new() -> Self {
        Self {
            recording: None,
            last: None,
        }
    }
}

static STATE: Mutex<State> = Mutex::new(State::new());
/// Gate barato lido por cada `record_system`/`record_script`.
static ON: AtomicBool = AtomicBool::new(false);

fn lock() -> std::sync::MutexGuard<'static, State> {
    STATE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// `true` enquanto há um trace a gravar.
pub fn is_recording() -> bool {
    ON.load(Ordering::Acquire)
}

/// Começa uma gravação; uma anterior é descartada. `max_seconds <= 0` grava
/// até [`stop`]. Devolve o estado (o mesmo payload de [`status`]).
pub fn start(max_seconds: f32, max_events: usize) -> Value {
    let max_events = max_events.clamp(1, MAX_EVENTS_CEILING);
    let started_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0);
    let mut state = lock();
    state.last = None;
    state.recording = Some(Recorder {
        start: Instant::now(),
        started_at,
        max_seconds: max_seconds.max(0.0),
        max_events,
        events: Vec::new(),
        frames: Vec::new(),
        dropped: 0,
        truncated: false,
    });
    ON.store(true, Ordering::Release);
    status_of(&state)
}

/// Fecha a gravação actual (idempotente). Devolve o estado; o trace fica
/// disponível para [`export`].
pub fn stop() -> Value {
    let mut state = lock();
    finish_locked(&mut state);
    status_of(&state)
}

/// Estado da gravação + resumo do último trace.
pub fn status() -> Value {
    status_of(&lock())
}

fn status_of(state: &State) -> Value {
    match &state.recording {
        Some(rec) => json!({
            "recording": true,
            "elapsed_s": rec.elapsed_s(),
            "max_seconds": rec.max_seconds,
            "frames": rec.frames.len(),
            "events": rec.events.len(),
            "max_events": rec.max_events,
            "truncated": rec.truncated,
            "dropped": rec.dropped,
            "available": false,
        }),
        None => json!({
            "recording": false,
            "frames": 0,
            "events": 0,
            "truncated": false,
            "dropped": 0,
            "available": state.last.is_some(),
            "last": state.last.as_ref().map(Finished::summary),
        }),
    }
}

fn finish_locked(state: &mut State) -> bool {
    let Some(rec) = state.recording.take() else {
        return false;
    };
    let duration_s = rec.elapsed_s();
    let mut events = rec.events;
    events.sort_by_key(|event| event.ts_us);
    let mut frames = rec.frames;
    frames.sort_by_key(|frame| frame.ts_us);
    let summary = analyze(
        &events,
        &frames,
        duration_s,
        rec.dropped,
        rec.truncated,
        rec.max_seconds,
        rec.max_events,
    );
    let finished = Finished {
        started_at: rec.started_at,
        duration_s,
        max_seconds: rec.max_seconds,
        max_events: rec.max_events,
        events,
        frames,
        dropped: rec.dropped,
        truncated: rec.truncated,
        summary,
    };
    state.last = Some(finished);
    ON.store(false, Ordering::Release);
    true
}

/// Registo de um sistema/âncora; chamado por [`super::timed::record_system`]
/// só quando a gravação está ligada.
pub fn record_system_event(group: Group, name: &'static str, ms: f32) {
    if !is_recording() || !ms.is_finite() {
        return;
    }
    let mut state = lock();
    let Some(rec) = state.recording.as_mut() else {
        return;
    };
    push_event(
        rec,
        Cow::Borrowed(name),
        group.as_str(),
        ms,
        EventKind::System,
    );
}

/// Registo de um escopo por script (`script/<ficheiro>`).
pub fn record_script_event(path: &str, ms: f32) {
    if !is_recording() || !ms.is_finite() {
        return;
    }
    let mut state = lock();
    let Some(rec) = state.recording.as_mut() else {
        return;
    };
    push_event(
        rec,
        Cow::Owned(path.to_string()),
        "scripts",
        ms,
        EventKind::Script,
    );
}

/// Marca explícita no trace (correlaciona um evento de jogo com a linha do
/// tempo). Devolve o frame actual, ou `None` sem gravação.
pub fn mark(label: &str) -> Option<u64> {
    if !is_recording() {
        return None;
    }
    let frame = super::timed::current_frame();
    let mut state = lock();
    let rec = state.recording.as_mut()?;
    let ts_us = rec.ts_us();
    if rec.events.len() >= rec.max_events {
        rec.truncated = true;
        rec.dropped += 1;
        return Some(frame);
    }
    rec.events.push(TraceEvent {
        frame,
        ts_us,
        dur_us: 0,
        name: Cow::Owned(label.to_string()),
        group: "mark",
        kind: EventKind::Mark,
    });
    Some(frame)
}

fn push_event(
    rec: &mut Recorder,
    name: Cow<'static, str>,
    group: &'static str,
    ms: f32,
    kind: EventKind,
) -> bool {
    if rec.events.len() >= rec.max_events {
        rec.truncated = true;
        rec.dropped += 1;
        return false;
    }
    let dur_us = (ms.max(0.0) * 1000.0).round().min(u64::MAX as f32) as u64;
    let end_us = rec.ts_us();
    rec.events.push(TraceEvent {
        frame: super::timed::current_frame(),
        ts_us: end_us.saturating_sub(dur_us),
        dur_us,
        name,
        group,
        kind,
    });
    true
}

/// Um frame concluído pelo main schedule. Chamado no `Last`; com duração
/// configurada fecha o trace sozinho. `hitch_top` só vem preenchido quando o
/// frame passou o limiar (a atribuição que o resumo publica); `counters`
/// entra nas amostras periódicas.
pub fn record_frame(frame: u64, ms: f32, hitch_top: Option<Value>, counters: Option<Value>) {
    if !is_recording() {
        return;
    }
    let hitch = hitch_top.is_some();
    let mut state = lock();
    let mut should_finish = false;
    if let Some(rec) = state.recording.as_mut() {
        if rec.frames.len() < MAX_FRAMES {
            let elapsed = rec.elapsed_s();
            rec.frames.push(TraceFrame {
                frame,
                ts_us: rec.ts_us(),
                ms,
                hitch,
                hitch_top,
                counters,
            });
            should_finish = rec.max_seconds > 0.0 && elapsed >= f64::from(rec.max_seconds);
        } else {
            rec.truncated = true;
            should_finish = true;
        }
    }
    if should_finish {
        finish_locked(&mut state);
    }
}

/// Agregado por sistema no trace.
#[derive(Default)]
struct SysAgg {
    total_ms: f64,
    max_ms: f64,
    calls: u64,
}

/// Análise pronta do trace — o que um agente/CLI precisa para decidir sem
/// abrir o Chrome trace nem tirar screenshots.
fn analyze(
    events: &[TraceEvent],
    frames: &[TraceFrame],
    duration_s: f64,
    dropped: u64,
    truncated: bool,
    max_seconds: f32,
    max_events: usize,
) -> Value {
    let mut frame_ms: Vec<f32> = frames.iter().map(|frame| frame.ms).collect();
    let total = frame_ms.len();
    let avg = (total > 0).then(|| frame_ms.iter().sum::<f32>() / total as f32);
    let min = frame_ms.iter().copied().reduce(f32::min);
    let max = frame_ms.iter().copied().reduce(f32::max);
    frame_ms.sort_by(f32::total_cmp);
    let percentile = |p: f64| -> Option<f32> {
        if frame_ms.is_empty() {
            return None;
        }
        let idx = (frame_ms.len() as f64 * p).ceil() as usize;
        Some(frame_ms[idx.saturating_sub(1).min(frame_ms.len() - 1)])
    };
    let over = |limit: f32| frame_ms.iter().filter(|&&ms| ms > limit).count();

    let mut systems: BTreeMap<(String, &'static str), SysAgg> = BTreeMap::new();
    let mut groups: BTreeMap<&'static str, f64> = BTreeMap::new();
    let mut marks = 0u64;
    for event in events {
        if event.kind == EventKind::Mark {
            marks += 1;
            continue;
        }
        let ms = event.dur_us as f64 / 1000.0;
        let agg = systems
            .entry((event.name.to_string(), event.group))
            .or_default();
        agg.total_ms += ms;
        agg.max_ms = agg.max_ms.max(ms);
        agg.calls += 1;
        *groups.entry(event.group).or_insert(0.0) += ms;
    }
    let wall_ms = (duration_s * 1000.0).max(1.0);
    let mut rows: Vec<(String, &'static str, SysAgg)> = systems
        .into_iter()
        .map(|((name, group), agg)| (name, group, agg))
        .collect();
    rows.sort_by(|a, b| b.2.total_ms.total_cmp(&a.2.total_ms));
    let row_json = |row: &(String, &'static str, SysAgg)| {
        json!({
            "name": row.0,
            "group": row.1,
            "total_ms": round2(row.2.total_ms),
            "max_ms": round2(row.2.max_ms),
            "calls": row.2.calls,
            "avg_ms": round2(row.2.total_ms / row.2.calls.max(1) as f64),
            "pct_wall": round2(row.2.total_ms / wall_ms * 100.0),
        })
    };
    let top_systems: Vec<Value> = rows.iter().take(15).map(row_json).collect();
    let mut by_max: Vec<&(String, &'static str, SysAgg)> = rows.iter().collect();
    by_max.sort_by(|a, b| b.2.max_ms.total_cmp(&a.2.max_ms));
    let top_max: Vec<Value> = by_max.iter().take(10).map(|row| row_json(row)).collect();

    let mut group_rows: Vec<(&'static str, f64)> = groups.into_iter().collect();
    group_rows.sort_by(|a, b| b.1.total_cmp(&a.1));
    let group_json: Vec<Value> = group_rows
        .iter()
        .map(|(group, ms)| {
            json!({
                "group": group,
                "total_ms": round2(*ms),
                "pct_wall": round2(*ms / wall_ms * 100.0),
            })
        })
        .collect();

    // Contadores: primeiro/último valor amostrado de cada chave (entidades,
    // chunks, scripts activos…) — a trend que explica uma subida de frame.
    let mut counters: BTreeMap<String, (Value, Value)> = BTreeMap::new();
    for frame in frames {
        if let Some(Value::Object(map)) = &frame.counters {
            for (key, value) in map {
                counters
                    .entry(key.clone())
                    .and_modify(|(_, last)| *last = value.clone())
                    .or_insert_with(|| (value.clone(), value.clone()));
            }
        }
    }
    let counters_json: serde_json::Map<String, Value> = counters
        .into_iter()
        .map(|(key, (first, last))| (key, json!({ "first": first, "last": last })))
        .collect();

    let recent_hitches: Vec<Value> = frames
        .iter()
        .rev()
        .filter(|frame| frame.hitch)
        .take(20)
        .map(|frame| {
            json!({
                "frame": frame.frame,
                "ms": round2(f64::from(frame.ms)),
                "top": frame.hitch_top,
            })
        })
        .collect();

    json!({
        "frames": frames.len(),
        "events": events.len(),
        "duration_s": round2(duration_s),
        "max_seconds": max_seconds,
        "max_events": max_events,
        "truncated": truncated,
        "dropped": dropped,
        "marks": marks,
        "frame_ms": {
            "avg": avg.map(|v| round2(f64::from(v))),
            "min": min.map(|v| round2(f64::from(v))),
            "max": max.map(|v| round2(f64::from(v))),
            "p50": percentile(0.50).map(|v| round2(f64::from(v))),
            "p95": percentile(0.95).map(|v| round2(f64::from(v))),
            "p99": percentile(0.99).map(|v| round2(f64::from(v))),
        },
        "over_budget": {
            "16_7_ms": over(16.7),
            "33_3_ms": over(33.3),
            "50_ms": over(50.0),
        },
        "hitches": {
            "count": frames.iter().filter(|frame| frame.hitch).count(),
            "recent": recent_hitches,
        },
        "top_systems": top_systems,
        "top_max": top_max,
        "groups": group_json,
        "counters": counters_json,
    })
}

fn round2(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

/// Resultado de um export.
#[derive(Debug, Clone, Serialize)]
pub struct TraceExport {
    pub path: String,
    pub bytes: u64,
    pub frames: usize,
    pub events: usize,
    pub truncated: bool,
}

/// Escreve o último trace (ou fecha o actual) para ficheiro.
///
/// `format` aceita `chrome`/`perfetto` (Trace Event Format) e `json` (formato
/// nativo). `path` ausente → `$TMPDIR/viber-traces/viber-trace-<epoch>.json`.
pub fn export(path: Option<PathBuf>, format: &str) -> std::io::Result<TraceExport> {
    let format = normalize_format(format)?;
    let finished = {
        let mut state = lock();
        finish_locked(&mut state);
        let Some(finished) = state.last.take() else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "nenhum trace gravado — usa `viber debug trace` ou o botão GRAVAR",
            ));
        };
        finished
    };
    let path = path.unwrap_or_else(default_path);
    // O `finished` saiu de `state.last`; se qualquer passo de IO falhar tem
    // de VOLTAR. Antes, um caminho inválido/disco cheio destruía a única
    // gravação e a retry dizia "nenhum trace gravado".
    let result = (|| -> std::io::Result<TraceExport> {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)?;
        }
        let file = std::fs::File::create(&path)?;
        let mut out = BufWriter::new(file);
        match format {
            "chrome" => write_chrome(&mut out, &finished)?,
            _ => write_native(&mut out, &finished)?,
        }
        out.flush()?;
        drop(out);
        let bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        Ok(TraceExport {
            path: path.display().to_string(),
            bytes,
            frames: finished.frames.len(),
            events: finished.events.len(),
            truncated: finished.truncated,
        })
    })();
    // Devolve o trace ao estado: exportar não o consome (dá para exportar
    // chrome e json da mesma gravação) e falhar também não.
    lock().last = Some(finished);
    result
}

fn normalize_format(format: &str) -> std::io::Result<&'static str> {
    match format.trim().to_ascii_lowercase().as_str() {
        "chrome" | "perfetto" | "trace" => Ok("chrome"),
        "json" | "native" | "viber" => Ok("json"),
        other => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("formato de trace desconhecido: {other} (chrome|json)"),
        )),
    }
}

fn default_path() -> PathBuf {
    let dir = std::env::temp_dir().join("viber-traces");
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    dir.join(format!("viber-trace-{secs}.json"))
}

// ---------------------------------------------------------------- chrome

#[derive(Serialize)]
struct ChromeEvent<'a> {
    name: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    cat: Option<&'a str>,
    ph: &'a str,
    pid: u32,
    tid: u32,
    ts: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    dur: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    s: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    args: Option<Value>,
}

fn write_chrome(out: &mut impl Write, finished: &Finished) -> std::io::Result<()> {
    out.write_all(b"{\"displayTimeUnit\":\"ms\",\"traceEvents\":[")?;
    let mut first = true;
    macro_rules! emit {
        ($event:expr) => {{
            if !first {
                out.write_all(b",")?;
            }
            first = false;
            serde_json::to_writer(&mut *out, &$event).map_err(io_json)?;
        }};
    }
    // Metadata: um processo e três threads virtuais (main/scripts/frame).
    emit!(ChromeEvent {
        name: "process_name",
        cat: None,
        ph: "M",
        pid: 1,
        tid: 0,
        ts: 0,
        dur: None,
        s: None,
        args: Some(json!({ "name": "Viber" })),
    });
    for (tid, name) in [(1u32, "main"), (2, "scripts"), (3, "frame")] {
        emit!(ChromeEvent {
            name: "thread_name",
            cat: None,
            ph: "M",
            pid: 1,
            tid,
            ts: 0,
            dur: None,
            s: None,
            args: Some(json!({ "name": name })),
        });
    }

    // Merge por timestamp: eventos e frames já vêm ordenados do finish.
    let (mut ei, mut fi) = (0usize, 0usize);
    while ei < finished.events.len() || fi < finished.frames.len() {
        let take_event = match (finished.events.get(ei), finished.frames.get(fi)) {
            (Some(event), Some(frame)) => event.ts_us <= frame.ts_us,
            (Some(_), None) => true,
            (None, Some(_)) => false,
            (None, None) => break,
        };
        if take_event {
            let event = &finished.events[ei];
            let ph = if event.kind == EventKind::Mark {
                "i"
            } else {
                "X"
            };
            emit!(ChromeEvent {
                name: event.name.as_ref(),
                cat: Some(event.group),
                ph,
                pid: 1,
                tid: event.kind.tid(),
                ts: event.ts_us,
                dur: (event.kind != EventKind::Mark).then_some(event.dur_us),
                s: (event.kind == EventKind::Mark).then_some("g"),
                args: (event.kind == EventKind::Mark).then(|| json!({ "frame": event.frame })),
            });
            ei += 1;
        } else {
            let frame = &finished.frames[fi];
            emit!(ChromeEvent {
                name: "frame_ms",
                cat: Some("frame"),
                ph: "C",
                pid: 1,
                tid: 3,
                ts: frame.ts_us,
                dur: None,
                s: None,
                args: Some(json!({ "value": frame.ms })),
            });
            if frame.hitch {
                emit!(ChromeEvent {
                    name: "hitch",
                    cat: Some("frame"),
                    ph: "i",
                    pid: 1,
                    tid: 3,
                    ts: frame.ts_us,
                    dur: None,
                    s: Some("g"),
                    args: Some(json!({ "ms": frame.ms, "frame": frame.frame })),
                });
            }
            if let Some(Value::Object(counters)) = &frame.counters {
                for (name, value) in counters {
                    emit!(ChromeEvent {
                        name,
                        cat: Some("counters"),
                        ph: "C",
                        pid: 1,
                        tid: 3,
                        ts: frame.ts_us,
                        dur: None,
                        s: None,
                        args: Some(json!({ "value": value })),
                    });
                }
            }
            fi += 1;
        }
    }
    out.write_all(b"]}")?;
    Ok(())
}

fn io_json(error: serde_json::Error) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::Other, error)
}

// ---------------------------------------------------------------- json

#[derive(Serialize)]
struct JsonEvent<'a> {
    frame: u64,
    ts_ms: f64,
    dur_ms: f64,
    name: &'a str,
    group: &'a str,
    kind: &'a str,
}

#[derive(Serialize)]
struct JsonFrame<'a> {
    frame: u64,
    ts_ms: f64,
    ms: f32,
    hitch: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    counters: Option<&'a Value>,
}

fn write_native(out: &mut impl Write, finished: &Finished) -> std::io::Result<()> {
    out.write_all(b"{\"tool\":\"viber\",\"kind\":\"profiler-trace\",")?;
    write!(
        out,
        "\"started_at\":{},\"duration_s\":{},\"max_seconds\":{},\"max_events\":{},\"truncated\":{},\"dropped\":{},",
        finished.started_at,
        finished.duration_s,
        finished.max_seconds,
        finished.max_events,
        finished.truncated,
        finished.dropped,
    )?;
    out.write_all(b"\"frames\":[")?;
    for (index, frame) in finished.frames.iter().enumerate() {
        if index > 0 {
            out.write_all(b",")?;
        }
        serde_json::to_writer(
            &mut *out,
            &JsonFrame {
                frame: frame.frame,
                ts_ms: frame.ts_us as f64 / 1000.0,
                ms: frame.ms,
                hitch: frame.hitch,
                counters: frame.counters.as_ref(),
            },
        )
        .map_err(io_json)?;
    }
    out.write_all(b"],\"events\":[")?;
    for (index, event) in finished.events.iter().enumerate() {
        if index > 0 {
            out.write_all(b",")?;
        }
        serde_json::to_writer(
            &mut *out,
            &JsonEvent {
                frame: event.frame,
                ts_ms: event.ts_us as f64 / 1000.0,
                dur_ms: event.dur_us as f64 / 1000.0,
                name: event.name.as_ref(),
                group: event.group,
                kind: event.kind.as_str(),
            },
        )
        .map_err(io_json)?;
    }
    out.write_all(b"]}")?;
    Ok(())
}

#[cfg(test)]
pub fn reset_for_tests() {
    ON.store(false, Ordering::Release);
    let mut state = lock();
    state.recording = None;
    state.last = None;
}

#[cfg(test)]
mod tests {
    use super::*;

    static TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn chrome_export_has_spans_marks_counters_and_hitch() {
        let _guard = TEST_LOCK.lock();
        reset_for_tests();
        start(10.0, 100);
        record_system_event(Group::Combat, "combat.melee", 2.0);
        record_script_event("enemies/wolf.lua", 0.5);
        mark("checkpoint").expect("marca gravada");
        record_frame(7, 16.0, None, Some(json!({ "entities": 100 })));
        record_frame(
            8,
            80.0,
            Some(json!([{ "name": "combat.melee", "ms": 40.0 }])),
            None,
        );
        let status = stop();
        assert_eq!(status["recording"], false);
        assert!(status["last"]["events"].as_u64().unwrap() >= 3);
        let summary = &status["last"];
        assert_eq!(summary["hitches"]["count"], 1, "{summary}");
        assert_eq!(summary["top_systems"][0]["name"], "combat.melee");
        assert!(summary["frame_ms"]["p95"].as_f64().is_some(), "{summary}");
        assert_eq!(summary["over_budget"]["16_7_ms"], 1, "{summary}");

        let dir = tempfile::tempdir().expect("tmpdir");
        let path = dir.path().join("trace.json");
        let exported = export(Some(path.clone()), "chrome").expect("export");
        assert_eq!(exported.path, path.display().to_string());
        assert!(exported.bytes > 0);
        let text = std::fs::read_to_string(&path).expect("ler trace");
        let value: Value = serde_json::from_str(&text).expect("json válido");
        let events = value["traceEvents"].as_array().expect("traceEvents");
        assert!(
            events
                .iter()
                .any(|event| { event["name"] == "process_name" && event["ph"] == "M" })
        );
        assert!(events.iter().any(|event| {
            event["name"] == "combat.melee"
                && event["cat"] == "combat"
                && event["ph"] == "X"
                && event["tid"] == 1
        }));
        assert!(
            events
                .iter()
                .any(|event| { event["name"] == "enemies/wolf.lua" && event["tid"] == 2 })
        );
        assert!(
            events
                .iter()
                .any(|event| { event["name"] == "checkpoint" && event["ph"] == "i" })
        );
        assert!(
            events
                .iter()
                .any(|event| { event["name"] == "frame_ms" && event["ph"] == "C" })
        );
        assert!(events.iter().any(|event| {
            event["name"] == "hitch" && event["args"]["ms"].as_f64() == Some(80.0)
        }));
        assert!(
            events
                .iter()
                .any(|event| { event["name"] == "entities" && event["ph"] == "C" })
        );
    }

    #[test]
    fn native_export_roundtrips() {
        let _guard = TEST_LOCK.lock();
        reset_for_tests();
        start(10.0, 100);
        record_system_event(Group::World, "world.day_cycle", 1.25);
        record_frame(3, 16.7, None, None);
        stop();
        let dir = tempfile::tempdir().expect("tmpdir");
        let path = dir.path().join("native.json");
        export(Some(path.clone()), "json").expect("export");
        let value: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).expect("ler")).expect("json");
        assert_eq!(value["kind"], "profiler-trace");
        assert_eq!(value["frames"][0]["frame"], 3);
        assert_eq!(value["events"][0]["name"], "world.day_cycle");
        assert_eq!(value["events"][0]["group"], "world");
        assert_eq!(value["events"][0]["kind"], "system");
    }

    #[test]
    fn auto_stop_after_duration() {
        let _guard = TEST_LOCK.lock();
        reset_for_tests();
        start(0.001, 100);
        std::thread::sleep(std::time::Duration::from_millis(5));
        record_frame(1, 16.0, None, None);
        assert!(!is_recording(), "a duração fecha o trace sozinha");
        let status = status();
        assert_eq!(status["available"], true);
        assert_eq!(status["last"]["frames"], 1);
    }

    #[test]
    fn event_cap_marks_truncated() {
        let _guard = TEST_LOCK.lock();
        reset_for_tests();
        start(10.0, 2);
        for _ in 0..5 {
            record_system_event(Group::Fx, "fx.particles", 0.1);
        }
        let status = status();
        assert_eq!(status["events"], 2);
        assert_eq!(status["truncated"], true);
        assert!(status["dropped"].as_u64().unwrap() >= 3);
        stop();
    }

    #[test]
    fn no_recording_means_no_work() {
        let _guard = TEST_LOCK.lock();
        reset_for_tests();
        record_system_event(Group::Fx, "fx.off", 1.0);
        assert_eq!(mark("sem trace"), None);
        assert_eq!(status()["available"], false);
    }

    #[test]
    fn export_without_trace_is_an_error() {
        let _guard = TEST_LOCK.lock();
        reset_for_tests();
        let error = export(None, "chrome").expect_err("sem trace");
        assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
    }
}
