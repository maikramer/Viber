//! d1-omni como serviço de decisão: ORT + embedding int8 + tokenizer.
//!
//! O grafo ONNX recebe `inputs_embeds` (o embedding fica fora, como no
//! `vael-nlu`) e devolve os logits de cada opção. O prompt é o `systemone`
//! (state + pergunta + opções com `<|mask|>`), portado de `prompt.py`.
//!
//! O contrato do artefacto (`<dir>`):
//! ```text
//! model.onnx / model.onnx.data   trunk + cabeça de decisão (int4/MatMulNBits)
//! embedding.bin                  VAELEMB1 (int8 por linha)
//! tokenizer.json                 vocabulário podado
//! decision_config.json           ids especiais, temperaturas, max_len
//! ```

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::OnceLock;

use anyhow::{Context, Result, anyhow, bail};
use regex::Regex;
use serde::Deserialize;
use serde_json::{Map, Value, json};
use tokenizers::Tokenizer;

// ------------------------------------------------------------------ config

fn default_threads() -> usize {
    8
}

#[derive(Debug, Clone, Deserialize)]
pub struct Special {
    pub bos: u32,
    pub state: u32,
    pub question: u32,
    pub option: u32,
    pub option_end: u32,
    pub decide: u32,
    pub mask: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub vocab_size: usize,
    pub hidden_size: usize,
    pub max_len: usize,
    pub special: Special,
    pub qtypes: HashMap<String, u8>,
    pub temperatures: HashMap<String, f32>,
    #[serde(default = "default_threads")]
    pub threads: usize,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ChoiceOption {
    pub key: String,
    #[serde(default)]
    pub description: Option<Value>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct NoulCriteria {
    #[serde(default, rename = "false")]
    pub no: Option<String>,
    #[serde(default, rename = "true")]
    pub yes: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Question {
    Choice {
        instructions: String,
        criteria: Vec<ChoiceOption>,
    },
    Score {
        instructions: String,
        criteria: Vec<String>,
    },
    Noul {
        instructions: String,
        #[serde(default)]
        criteria: Option<NoulCriteria>,
    },
}

impl Question {
    fn kind(&self) -> &'static str {
        match self {
            Question::Choice { .. } => "choice",
            Question::Score { .. } => "score",
            Question::Noul { .. } => "noul",
        }
    }

    fn instructions(&self) -> &str {
        match self {
            Question::Choice { instructions, .. }
            | Question::Score { instructions, .. }
            | Question::Noul { instructions, .. } => instructions,
        }
    }

    fn n_options(&self) -> usize {
        match self {
            Question::Choice { criteria, .. } => criteria.len(),
            Question::Score { criteria, .. } => criteria.len(),
            Question::Noul { .. } => 2,
        }
    }

    fn options(&self) -> Vec<String> {
        match self {
            Question::Choice { criteria, .. } => criteria
                .iter()
                .map(|o| match o.description.as_ref() {
                    None => o.key.clone(),
                    Some(Value::String(s)) if s.is_empty() => o.key.clone(),
                    Some(v) => format!("{}: {}", o.key, criterion(v)),
                })
                .collect(),
            Question::Score { criteria, .. } => criteria
                .iter()
                .enumerate()
                .map(|(i, c)| format!("level {i}: {}", criterion(&Value::String(c.clone()))))
                .collect(),
            Question::Noul { criteria, .. } => {
                let (no, yes) = match criteria {
                    Some(c) => (c.no.clone(), c.yes.clone()),
                    None => (None, None),
                };
                vec![
                    format!(
                        "false: {}",
                        no.unwrap_or_else(|| "no, the statement does not hold".into())
                    ),
                    format!(
                        "true: {}",
                        yes.unwrap_or_else(|| "yes, the statement holds".into())
                    ),
                ]
            }
        }
    }
}

// ------------------------------------------------------------------ prompt

struct Prompt {
    ids: Vec<u32>,
    markers: Vec<usize>,
}

fn special_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"<\|([A-Za-z0-9_]+)\|>").expect("regex"))
}

/// `<|name|>` -> `<¦name¦>`: texto do chamador não pode forjar um delimitador.
fn escape(text: &str) -> String {
    special_re().replace_all(text, "<¦$1¦>").into_owned()
}

/// `json.dumps(ensure_ascii=False)` do Python (separadores ", " e ": ").
fn json_spaced(v: &Value) -> String {
    match v {
        Value::Null => "null".into(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into()),
        Value::Array(a) => format!(
            "[{}]",
            a.iter().map(json_spaced).collect::<Vec<_>>().join(", ")
        ),
        Value::Object(o) => format!(
            "{{{}}}",
            o.iter()
                .map(|(k, v)| format!("{}: {}", serde_json::to_string(k).unwrap(), json_spaced(v)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

fn criterion(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => json_spaced(other),
    }
}

fn serialize_state(state: &Value) -> String {
    match state {
        Value::String(s) => s.clone(),
        other => json_spaced(other),
    }
}

fn encode(tok: &Tokenizer, text: &str) -> Result<Vec<u32>> {
    let enc = tok
        .encode(escape(text), false)
        .map_err(|e| anyhow!("tokenizer: {e}"))?;
    Ok(enc.get_ids().to_vec())
}

fn build_prompt(tok: &Tokenizer, cfg: &Config, state: &Value, q: &Question) -> Result<Prompt> {
    let opts = q.options();
    if opts.len() < 2 {
        bail!("a pergunta {:?} precisa de pelo menos 2 opções", q.kind());
    }
    let max_len = cfg.max_len;
    let budget = (opts.len() * 24 + 32).max(96).min(max_len / 2);
    let per = (budget.saturating_sub(3 * opts.len()) / opts.len()).max(2);

    let mut question = vec![cfg.special.question];
    question.extend(
        encode(tok, q.instructions())?
            .into_iter()
            .take(budget.max(16)),
    );
    let mut markers = Vec::with_capacity(opts.len());
    for text in &opts {
        markers.push(question.len() + 1);
        question.push(cfg.special.option);
        question.push(cfg.special.mask);
        question.extend(encode(tok, &format!(" {text}"))?.into_iter().take(per));
        question.push(cfg.special.option_end);
    }
    question.push(cfg.special.decide);

    let room = max_len.saturating_sub(question.len() + 2);
    let mut state_ids = vec![cfg.special.state];
    state_ids.extend(encode(tok, &serialize_state(state))?.into_iter().take(room));

    let mut ids = Vec::with_capacity(1 + state_ids.len() + question.len());
    ids.push(cfg.special.bos);
    ids.extend_from_slice(&state_ids);
    ids.extend_from_slice(&question);
    ids.truncate(max_len);

    let shift = 1 + state_ids.len();
    for m in &mut markers {
        *m += shift;
        if *m >= max_len {
            bail!("as opções não cabem no contexto ({max_len})");
        }
    }
    Ok(Prompt { ids, markers })
}

// ------------------------------------------------------------------ answer

fn softmax(logits: &[f32], temperature: f32) -> Vec<f64> {
    let t = temperature.max(1e-6) as f64;
    let max = logits.iter().cloned().fold(f32::NEG_INFINITY, f32::max) as f64;
    let exps: Vec<f64> = logits
        .iter()
        .map(|x| (((*x as f64) - max) / t).exp())
        .collect();
    let sum: f64 = exps.iter().sum();
    exps.into_iter().map(|e| e / sum).collect()
}

fn temperature_key(q: &Question) -> String {
    let n = q.n_options();
    let bucket = if n <= 2 {
        "2"
    } else if n <= 5 {
        "3-5"
    } else if n <= 10 {
        "6-10"
    } else {
        "11+"
    };
    format!("{}:{bucket}", q.kind())
}

fn format_answer(cfg: &Config, q: &Question, logits: &[f32]) -> Value {
    let key = temperature_key(q);
    let temp = cfg
        .temperatures
        .get(&key)
        .or_else(|| cfg.temperatures.get(q.kind()))
        .copied()
        .unwrap_or(1.0);
    let p = softmax(logits, temp);
    match q {
        Question::Noul { .. } => json!({"type": "noul", "noul": p[1]}),
        Question::Choice { criteria, .. } => {
            let best = (0..p.len()).fold(0usize, |b, i| if p[i] > p[b] { i } else { b });
            let mut probs = Map::new();
            for (o, v) in criteria.iter().zip(p.iter()) {
                probs.insert(o.key.clone(), json!(v));
            }
            json!({
                "type": "choice",
                "choice": criteria[best].key,
                "confidence": p[best],
                "probabilities": probs,
            })
        }
        Question::Score { criteria, .. } => {
            let score: f64 = p.iter().enumerate().map(|(i, v)| i as f64 * v).sum();
            let best = (0..p.len()).fold(0usize, |b, i| if p[i] > p[b] { i } else { b });
            let mut probs = Map::new();
            for (i, v) in p.iter().enumerate() {
                probs.insert(i.to_string(), json!(v));
            }
            let legend: Map<String, Value> = criteria
                .iter()
                .enumerate()
                .map(|(i, c)| (i.to_string(), json!(c)))
                .collect();
            json!({
                "type": "score",
                "score": score,
                "confidence": p[best],
                "probabilities": probs,
                "legend": legend,
            })
        }
    }
}

// ------------------------------------------------------------------ runtime

pub struct Runtime {
    pub cfg: Config,
    tok: Tokenizer,
    emb: vael_nlu::Embedding,
    session: ort::session::Session,
    _env: ort::environment::Environment,
}

impl Runtime {
    pub fn load(dir: impl AsRef<Path>) -> Result<Self> {
        let dir = dir.as_ref();
        let cfg: Config = serde_json::from_str(
            &fs::read_to_string(dir.join("decision_config.json"))
                .with_context(|| format!("decision_config.json em {}", dir.display()))?,
        )?;
        let tok = Tokenizer::from_file(dir.join("tokenizer.json"))
            .map_err(|e| anyhow!("tokenizer: {e}"))?;
        let emb = vael_nlu::load_embedding(&dir.join("embedding.bin"))?;
        if emb.rows != cfg.vocab_size {
            bail!(
                "embedding tem {} linhas, config.vocab_size={}",
                emb.rows,
                cfg.vocab_size
            );
        }
        if emb.dim != cfg.hidden_size {
            bail!(
                "embedding dim {} != config.hidden_size={}",
                emb.dim,
                cfg.hidden_size
            );
        }
        if tok.get_vocab_size(true) > emb.rows {
            bail!(
                "tokenizer tem {} tokens, embedding só {} linhas",
                tok.get_vocab_size(true),
                emb.rows
            );
        }

        let env = ort::init()
            .with_name("vael-decision")
            .build()
            .map_err(|e| anyhow!("{e:?}"))?;
        let builder = ort::session::Session::builder(&env).map_err(|e| anyhow!("{e:?}"))?;
        let builder = builder
            .with_optimization_level(ort::session::builder::GraphOptimizationLevel::All)
            .map_err(|e| anyhow!("{e:?}"))?;
        let builder = builder
            .with_intra_threads(cfg.threads.max(1))
            .map_err(|e| anyhow!("{e:?}"))?;
        let builder = builder
            .with_inter_threads(1)
            .map_err(|e| anyhow!("{e:?}"))?;
        let mut builder = builder;
        let session = builder
            .commit_from_file(dir.join("model.onnx"))
            .map_err(|e| anyhow!("model.onnx: {e:?}"))?;

        Ok(Self {
            cfg,
            tok,
            emb,
            session,
            _env: env,
        })
    }

    /// `questions` é um mapa nome -> pergunta (`choice`/`score`/`noul`).
    pub fn decide(&mut self, state: &Value, questions: &Map<String, Value>) -> Result<Value> {
        if questions.is_empty() {
            bail!("questões vazias");
        }
        let mut answers = Map::new();
        let mut tokens = 0usize;
        for (name, raw) in questions {
            let q: Question = serde_json::from_value(raw.clone())
                .with_context(|| format!("pergunta {name:?} inválida"))?;
            let qtype = *self
                .cfg
                .qtypes
                .get(q.kind())
                .ok_or_else(|| anyhow!("qtype em falta para {:?}", q.kind()))?;
            let prompt = build_prompt(&self.tok, &self.cfg, state, &q)?;
            let logits = self.run(&prompt.ids, &prompt.markers, qtype)?;
            tokens += prompt.ids.len();
            answers.insert(name.clone(), format_answer(&self.cfg, &q, &logits));
        }
        Ok(json!({
            "answers": answers,
            "usage": {"input_tokens": tokens, "output_tokens": 0},
        }))
    }

    fn run(&mut self, ids: &[u32], markers: &[usize], qtype: u8) -> Result<Vec<f32>> {
        let n = ids.len();
        let h = self.cfg.hidden_size;
        let k = markers.len();
        let mut emb = Vec::new();
        self.emb.gather_into(ids, &mut emb);
        let pad = vec![true; n];
        let marker_pos: Vec<i64> = markers.iter().map(|&m| m as i64).collect();
        let marker_mask = vec![true; k];
        let qtype = [qtype as i64];

        let outputs = self
            .session
            .run(ort::inputs![
                ort::value::TensorRef::from_array_view(([1usize, n, h], &emb[..]))
                    .map_err(|e| anyhow!("{e:?}"))?,
                ort::value::TensorRef::from_array_view(([1usize, n], &pad[..]))
                    .map_err(|e| anyhow!("{e:?}"))?,
                ort::value::TensorRef::from_array_view(([1usize, k], &marker_pos[..]))
                    .map_err(|e| anyhow!("{e:?}"))?,
                ort::value::TensorRef::from_array_view(([1usize, k], &marker_mask[..]))
                    .map_err(|e| anyhow!("{e:?}"))?,
                ort::value::TensorRef::from_array_view(([1usize], &qtype[..]))
                    .map_err(|e| anyhow!("{e:?}"))?,
            ])
            .map_err(|e| anyhow!("{e:?}"))?;
        if outputs.len() != 1 {
            bail!("modelo devolveu {} saídas, esperava 1", outputs.len());
        }
        let (shape, data) = outputs[0]
            .try_extract_tensor::<f32>()
            .map_err(|e| anyhow!("{e:?}"))?;
        if shape.len() != 2 || shape[0] != 1 || shape[1] != k as i64 {
            bail!("logits com forma {shape:?}, esperava [1, {k}]");
        }
        Ok(data.to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn artifact() -> Option<std::path::PathBuf> {
        let dir = std::env::var("VAEL_DECISION_ARTIFACT").unwrap_or_else(|_| {
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../../Vael/assets/public/d1"
            )
            .to_string()
        });
        let dir = std::path::PathBuf::from(dir);
        if dir.join("model.onnx").is_file() && dir.join("model.onnx.data").is_file() {
            Some(dir)
        } else {
            eprintln!("skip: artefacto ausente em {}", dir.display());
            None
        }
    }

    #[test]
    fn card_example_routes_to_billing() {
        let Some(dir) = artifact() else { return };
        let mut rt = Runtime::load(&dir).unwrap();
        let state = json!("I was charged twice for my order last week and nobody has replied.");
        let q = json!({
            "type": "choice",
            "instructions": "Which team should handle this?",
            "criteria": [
                {"key": "billing", "description": "payments and refunds"},
                {"key": "shipping"},
                {"key": "technical"}
            ]
        });
        let mut qs = Map::new();
        qs.insert("route".into(), q);
        let out = rt.decide(&state, &qs).unwrap();
        assert_eq!(out["answers"]["route"]["choice"], "billing", "{out}");
        assert!(out["answers"]["route"]["confidence"].as_f64().unwrap() > 0.5);
    }
}
