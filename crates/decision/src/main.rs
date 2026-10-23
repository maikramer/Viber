//! QA/benchmark do artefacto de decisão, fora da engine.
//! uso: vael-decision <dir> [iterações]

use std::time::Instant;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let dir = args.next().unwrap_or_else(|| "assets/public/d1".into());
    let iters: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(1);

    let state =
        serde_json::json!("I was charged twice for my order last week and nobody has replied.");
    let questions = serde_json::json!({
        "route": {
            "type": "choice",
            "instructions": "Which team should handle this?",
            "criteria": [
                {"key": "billing", "description": "payments and refunds"},
                {"key": "shipping"},
                {"key": "technical"}
            ]
        }
    });
    let questions = questions.as_object().unwrap().clone();

    let t0 = Instant::now();
    let mut rt = vael_decision::Runtime::load(&dir)?;
    eprintln!("load {:.0} ms", t0.elapsed().as_secs_f64() * 1000.0);

    let mut lat = Vec::with_capacity(iters);
    let mut last = serde_json::Value::Null;
    for _ in 0..iters.max(1) {
        let t = Instant::now();
        last = rt.decide(&state, &questions)?;
        lat.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    lat.sort_by(|a, b| a.partial_cmp(b).unwrap());
    println!("{}", serde_json::to_string_pretty(&last)?);
    if lat.len() > 1 {
        eprintln!(
            "p50 {:.0} ms  p95 {:.0} ms  (n={})",
            lat[lat.len() / 2],
            lat[(lat.len() * 95 / 100).min(lat.len() - 1)],
            lat.len()
        );
    }
    Ok(())
}
