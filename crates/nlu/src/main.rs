use anyhow::Result;
use std::io::BufRead;
use std::time::Instant;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let dir = args.get(1).cloned().unwrap_or_else(|| "out/artifact".into());

    if args.get(2).map(|s| s.as_str()) == Some("--bench") {
        let path = args.get(3).cloned().unwrap_or_else(|| "bench/synth_test.jsonl".into());
        let out = args.get(4).cloned();
        return bench(&dir, &path, out);
    }

    let text = if args.len() > 2 {
        args[2..].join(" ")
    } else {
        let mut s = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut s)?;
        s.trim().to_string()
    };
    let mut rt = vael_nlu::Runtime::load(&dir)?;
    let out = rt.parse(&text)?;
    println!("{}", serde_json::to_string_pretty(&out)?);
    Ok(())
}

fn bench(dir: &str, path: &str, out: Option<String>) -> Result<()> {
    let t0 = Instant::now();
    let mut rt = vael_nlu::Runtime::load(dir)?;
    eprintln!("load {:.0} ms", t0.elapsed().as_secs_f64() * 1000.0);

    let f = std::fs::File::open(path)?;
    let mut times = Vec::new();
    let mut lines = Vec::new();
    for line in std::io::BufReader::new(f).lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let v: serde_json::Value = serde_json::from_str(&line)?;
        let text = v["text"].as_str().unwrap_or_default().to_string();
        let t = Instant::now();
        let parsed = rt.parse(&text)?;
        times.push(t.elapsed().as_secs_f64() * 1000.0);
        lines.push(serde_json::json!({
            "text": text,
            "entities": parsed.entities,
            "intent": parsed.intent,
            "intent_score": parsed.intent_score,
        }));
    }
    let mut sorted = times.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = sorted.len();
    if n == 0 {
        // Corpus vazio (ou só linhas em branco): não há percentis. Antes
        // rebentava em `sorted[n / 2]` com exit 101.
        eprintln!("n=0 — nenhuma frase em {path}");
    } else {
        // Percentil nearest-rank: índice ceil(0.95*n)-1. `floor(...)-1`
        // devolvia o mínimo para n=2 e ficava um posto abaixo no resto.
        let p95 = ((n as f64 * 0.95).ceil() as usize).saturating_sub(1).min(n - 1);
        eprintln!(
            "n={n} median {:.2} ms p95 {:.2} ms mean {:.2} ms",
            sorted[n / 2],
            sorted[p95],
            times.iter().sum::<f64>() / n as f64
        );
    }
    let body = lines.iter().map(|v| serde_json::to_string(v).unwrap()).collect::<Vec<_>>().join("\n");
    match out {
        Some(p) => std::fs::write(p, body)?,
        None => println!("{body}"),
    }
    Ok(())
}
