//! Aba **FRAMES** do profiler — a leitura que liga o frame médio ao frame que
//! engasgou.
//!
//! * janela de frame-times (média/p95/pior) + a série completa para o
//!   sparkline do painel;
//! * topo do frame actual (sistemas + scripts, por ms);
//! * hitches recentes com o topo que os causou;
//! * memória do processo e estado do gravador de traces.

use bevy::prelude::*;
use serde_json::{Value, json};

use super::{FrameStats, HitchLog, hitch_json, mem, timed, trace};

/// Snapshot da aba (`viber.profiler.tab {"tab":"frames"}`).
pub fn snapshot(world: &World) -> Value {
    let stats = world.get_resource::<FrameStats>();
    let (avg, min, max, p95) = stats
        .map(FrameStats::window)
        .unwrap_or((None, None, None, None));
    let history = stats.map(FrameStats::history).unwrap_or_default();
    let window_frames = stats.map(FrameStats::len).unwrap_or(0);
    let frame_avg = avg.unwrap_or(0.0);

    let top: Vec<Value> = timed::systems_snapshot(frame_avg)
        .into_iter()
        .take(12)
        .filter_map(|row| serde_json::to_value(row).ok())
        .collect();
    let hitches = world
        .get_resource::<HitchLog>()
        .map(hitch_json)
        .unwrap_or_else(
            || json!({ "threshold_ms": super::DEFAULT_HITCH_MS, "count": 0, "recent": [] }),
        );

    json!({
        "frame_ms": {
            "avg": avg,
            "min": min,
            "max": max,
            "p95": p95,
        },
        "window_frames": window_frames,
        "history": history,
        "top": top,
        "hitches": hitches,
        "memory": mem::snapshot(),
        "trace": trace::status(),
    })
}
