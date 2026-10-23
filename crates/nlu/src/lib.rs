//! Vael NLU runtime: GLiNER2 span extraction + intent classification.
//!
//! Deployment artifact (see tools/nlu/scripts/build_artifact.py):
//!   encoder.onnx     pruned DeBERTa, 4-bit weight-only (inputs_embeds -> states)
//!   embedding.bin    pruned int8 token embedding
//!   heads.bin        span head, count head and classifier (int8 per channel)
//!   tokenizer.json   pruned Unigram tokenizer
//!   nlu_config.json  ids, labels, thresholds

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use anyhow::{anyhow, bail, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub vocab_size: usize,
    pub unk_id: u32,
    pub specials: HashMap<String, u32>,
    pub hidden_size: usize,
    pub max_width: usize,
    pub entity_labels: Vec<String>,
    pub intent_labels: Vec<String>,
    pub entity_map: HashMap<String, String>,
    pub threshold: f32,
    #[serde(default = "default_intent_threshold")]
    pub intent_threshold: f32,
    pub max_tokens: usize,
    pub max_count: usize,
    #[serde(default = "default_threads")]
    pub threads: usize,
}

fn default_threads() -> usize {
    8
}

fn default_intent_threshold() -> f32 {
    0.5
}

/// Fallback portable, usado fora de x86_64 e em x86_64 sem AVX2/FMA.
mod scalar {
    #[inline]
    pub fn dot_i8_f32(w: &[i8], x: &[f32]) -> f32 {
        w.iter().zip(x).map(|(a, b)| *a as f32 * *b).sum()
    }

    #[inline]
    pub fn dot4_i8_f32(w: &[i8], x0: &[f32], x1: &[f32], x2: &[f32], x3: &[f32]) -> [f32; 4] {
        [
            w.iter().zip(x0).map(|(a, b)| *a as f32 * *b).sum(),
            w.iter().zip(x1).map(|(a, b)| *a as f32 * *b).sum(),
            w.iter().zip(x2).map(|(a, b)| *a as f32 * *b).sum(),
            w.iter().zip(x3).map(|(a, b)| *a as f32 * *b).sum(),
        ]
    }

    pub fn ger_transposed(w: &[i8], scale: &[f32], rows: usize, cols: usize, x: &[f32], y: &mut [f32]) {
        for i in 0..rows {
            let xi = x[i] * scale[i];
            let row = &w[i * cols..(i + 1) * cols];
            for (o, q) in row.iter().enumerate() {
                y[o] += xi * *q as f32;
            }
        }
    }
}

#[cfg(target_arch = "x86_64")]
mod simd {
    use std::arch::x86_64::*;

    use super::scalar;

    /// `#[target_feature(enable = "avx2,fma")]` numa CPU sem os dois bits é
    /// SIGILL (abort), não um fallback: a detecção tem de ser explícita.
    #[inline]
    fn avx2_fma() -> bool {
        std::arch::is_x86_feature_detected!("avx2") && std::arch::is_x86_feature_detected!("fma")
    }

    #[inline]
    pub fn dot_i8_f32(w: &[i8], x: &[f32]) -> f32 {
        if avx2_fma() {
            unsafe { dot_i8_f32_avx2(w, x) }
        } else {
            scalar::dot_i8_f32(w, x)
        }
    }

    #[target_feature(enable = "avx2,fma")]
    unsafe fn dot_i8_f32_avx2(w: &[i8], x: &[f32]) -> f32 {
        let n = w.len().min(x.len());
        let mut i = 0usize;
        let mut acc = _mm256_setzero_ps();
        while i + 8 <= n {
            let w8 = _mm_loadl_epi64(w.as_ptr().add(i) as *const __m128i);
            let wf = _mm256_cvtepi32_ps(_mm256_cvtepi8_epi32(w8));
            let xv = _mm256_loadu_ps(x.as_ptr().add(i));
            acc = _mm256_fmadd_ps(wf, xv, acc);
            i += 8;
        }
        let hi = _mm256_extractf128_ps(acc, 1);
        let lo = _mm256_castps256_ps128(acc);
        let mut s = _mm_cvtss_f32(_mm_add_ps(hi, lo));
        // horizontal sum of the 4 remaining lanes
        let mut tmp = [0.0f32; 4];
        _mm_storeu_ps(tmp.as_mut_ptr(), _mm_add_ps(hi, lo));
        s = tmp.iter().sum();
        while i < n {
            s += w[i] as f32 * x[i];
            i += 1;
        }
        s
    }

    /// Four vectors at once, reusing each weight load.
    #[inline]
    pub fn dot4_i8_f32(w: &[i8], x0: &[f32], x1: &[f32], x2: &[f32], x3: &[f32]) -> [f32; 4] {
        if avx2_fma() {
            unsafe { dot4_avx2(w, x0, x1, x2, x3) }
        } else {
            scalar::dot4_i8_f32(w, x0, x1, x2, x3)
        }
    }

    #[target_feature(enable = "avx2,fma")]
    unsafe fn dot4_avx2(w: &[i8], x0: &[f32], x1: &[f32], x2: &[f32], x3: &[f32]) -> [f32; 4] {
        let n = w.len();
        let mut i = 0usize;
        let mut a0 = _mm256_setzero_ps();
        let mut a1 = _mm256_setzero_ps();
        let mut a2 = _mm256_setzero_ps();
        let mut a3 = _mm256_setzero_ps();
        while i + 8 <= n {
            let w8 = _mm_loadl_epi64(w.as_ptr().add(i) as *const __m128i);
            let wf = _mm256_cvtepi32_ps(_mm256_cvtepi8_epi32(w8));
            a0 = _mm256_fmadd_ps(wf, _mm256_loadu_ps(x0.as_ptr().add(i)), a0);
            a1 = _mm256_fmadd_ps(wf, _mm256_loadu_ps(x1.as_ptr().add(i)), a1);
            a2 = _mm256_fmadd_ps(wf, _mm256_loadu_ps(x2.as_ptr().add(i)), a2);
            a3 = _mm256_fmadd_ps(wf, _mm256_loadu_ps(x3.as_ptr().add(i)), a3);
            i += 8;
        }
        let hs = |v: __m256| -> f32 {
            let lo = _mm256_castps256_ps128(v);
            let hi = _mm256_extractf128_ps(v, 1);
            let mut t = [0.0f32; 4];
            _mm_storeu_ps(t.as_mut_ptr(), _mm_add_ps(lo, hi));
            t.iter().sum()
        };
        let mut r = [hs(a0), hs(a1), hs(a2), hs(a3)];
        while i < n {
            let wv = w[i] as f32;
            r[0] += wv * x0[i];
            r[1] += wv * x1[i];
            r[2] += wv * x2[i];
            r[3] += wv * x3[i];
            i += 1;
        }
        r
    }

    /// y[o] += x[i] * w[i, o] * scale[i] over all rows i (contiguous rows).
    pub fn ger_transposed(w: &[i8], scale: &[f32], rows: usize, cols: usize, x: &[f32], y: &mut [f32]) {
        if avx2_fma() {
            unsafe { ger_transposed_avx2(w, scale, rows, cols, x, y) }
        } else {
            scalar::ger_transposed(w, scale, rows, cols, x, y)
        }
    }

    #[target_feature(enable = "avx2,fma")]
    unsafe fn ger_transposed_avx2(
        w: &[i8], scale: &[f32], rows: usize, cols: usize, x: &[f32], y: &mut [f32],
    ) {
        for i in 0..rows {
            let xi = x[i] * scale[i];
            if xi == 0.0 {
                continue;
            }
            let row = w.as_ptr().add(i * cols);
            let mut o = 0usize;
            let xv = _mm256_set1_ps(xi);
            while o + 8 <= cols {
                let w8 = _mm_loadl_epi64(row.add(o) as *const __m128i);
                let wf = _mm256_cvtepi32_ps(_mm256_cvtepi8_epi32(w8));
                let cur = _mm256_loadu_ps(y.as_ptr().add(o));
                _mm256_storeu_ps(y.as_mut_ptr().add(o), _mm256_fmadd_ps(xv, wf, cur));
                o += 8;
            }
            while o < cols {
                *y.get_unchecked_mut(o) += xi * (*row.add(o) as f32);
                o += 1;
            }
        }
    }
}

#[cfg(not(target_arch = "x86_64"))]
mod simd {
    pub use super::scalar::{dot4_i8_f32, dot_i8_f32, ger_transposed};
}

/// A weight-only quantized matrix stored as i8 values plus one scale per row.
pub struct QMatrix {
    pub rows: usize,
    pub cols: usize,
    pub q: Vec<i8>,
    pub scale: Vec<f32>,
}

impl QMatrix {
    /// y[o] = dot(W[o], x) * scale[o] + bias[o]
    pub fn mv_into(&self, x: &[f32], bias: Option<&[f32]>, y: &mut [f32]) {
        debug_assert_eq!(x.len(), self.cols);
        debug_assert_eq!(y.len(), self.rows);
        for o in 0..self.rows {
            let row = &self.q[o * self.cols..(o + 1) * self.cols];
            let acc = simd::dot_i8_f32(row, x);
            let b = bias.map_or(0.0, |b| b[o]);
            y[o] = acc * self.scale[o] + b;
        }
    }

    /// Same as `mv_into`, but uses a column slice of a wider matrix, which is
    /// how the span head splits the fused 2H->4H projection.
    pub fn mv_cols_into(&self, x: &[f32], col0: usize, ncols: usize, y: &mut [f32]) {
        for o in 0..self.rows {
            let base = o * self.cols + col0;
            let row = &self.q[base..base + ncols];
            y[o] = simd::dot_i8_f32(row, x) * self.scale[o];
        }
    }

    /// X [n, cols] · Wᵀ -> Y [n, rows], four rows at a time (weight reuse).
    pub fn mm4_into(&self, x: &[f32], n: usize, bias: Option<&[f32]>, y: &mut [f32]) {
        let cols = self.cols;
        let mut i = 0usize;
        while i + 4 <= n {
            let (x0, x1, x2, x3) = (
                &x[i * cols..(i + 1) * cols],
                &x[(i + 1) * cols..(i + 2) * cols],
                &x[(i + 2) * cols..(i + 3) * cols],
                &x[(i + 3) * cols..(i + 4) * cols],
            );
            for o in 0..self.rows {
                let row = &self.q[o * cols..(o + 1) * cols];
                let r = simd::dot4_i8_f32(row, x0, x1, x2, x3);
                let sc = self.scale[o];
                let b = bias.map_or(0.0, |b| b[o]);
                y[i * self.rows + o] = r[0] * sc + b;
                y[(i + 1) * self.rows + o] = r[1] * sc + b;
                y[(i + 2) * self.rows + o] = r[2] * sc + b;
                y[(i + 3) * self.rows + o] = r[3] * sc + b;
            }
            i += 4;
        }
        if i < n {
            self.mm_into(&x[i * cols..], n - i, bias, &mut y[i * self.rows..]);
        }
    }

    /// Like `mm4_into`, but only columns [col0, col0+ncols) of W.
    pub fn mm4_cols_into(&self, x: &[f32], n: usize, col0: usize, ncols: usize, y: &mut [f32]) {
        let cols = self.cols;
        let mut i = 0usize;
        while i + 4 <= n {
            let (x0, x1, x2, x3) = (
                &x[i * ncols..(i + 1) * ncols],
                &x[(i + 1) * ncols..(i + 2) * ncols],
                &x[(i + 2) * ncols..(i + 3) * ncols],
                &x[(i + 3) * ncols..(i + 4) * ncols],
            );
            for o in 0..self.rows {
                let base = o * cols + col0;
                let row = &self.q[base..base + ncols];
                let r = simd::dot4_i8_f32(row, x0, x1, x2, x3);
                let sc = self.scale[o];
                y[i * self.rows + o] = r[0] * sc;
                y[(i + 1) * self.rows + o] = r[1] * sc;
                y[(i + 2) * self.rows + o] = r[2] * sc;
                y[(i + 3) * self.rows + o] = r[3] * sc;
            }
            i += 4;
        }
        if i < n {
            for j in i..n {
                self.mv_cols_into(&x[j * ncols..(j + 1) * ncols], col0, ncols,
                                  &mut y[j * self.rows..(j + 1) * self.rows]);
            }
        }
    }

    /// Y [cols] = X [rows] · W, i.e. W stored row-major [rows, cols].
    pub fn gemv_transposed(&self, x: &[f32], y: &mut [f32]) {
        y.fill(0.0);
        simd::ger_transposed(&self.q, &self.scale, self.rows, self.cols, x, y);
    }

    /// X [n, cols] · Wᵀ -> Y [n, rows].
    pub fn mm_into(&self, x: &[f32], n: usize, bias: Option<&[f32]>, y: &mut [f32]) {
        let mut row = vec![0.0f32; self.cols];
        for i in 0..n {
            row.copy_from_slice(&x[i * self.cols..(i + 1) * self.cols]);
            self.mv_into(&row, bias, &mut y[i * self.rows..(i + 1) * self.rows]);
        }
    }
}

pub struct Embedding {
    pub rows: usize,
    pub dim: usize,
    pub q: Vec<i8>,
    pub scale: Vec<f32>,
}

impl Embedding {
    /// Gathers and dequantizes `ids` into a [ids.len(), dim] row-major buffer.
    pub fn gather_into(&self, ids: &[u32], out: &mut Vec<f32>) {
        out.resize(ids.len() * self.dim, 0.0);
        for (i, id) in ids.iter().enumerate() {
            let id = *id as usize;
            let s = self.scale[id];
            let src = &self.q[id * self.dim..(id + 1) * self.dim];
            let dst = &mut out[i * self.dim..(i + 1) * self.dim];
            for (d, q) in dst.iter_mut().zip(src.iter()) {
                *d = *q as f32 * s;
            }
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Entity {
    pub label: String,
    pub start: usize,
    pub end: usize,
    pub text: String,
    pub score: f32,
}

#[derive(Debug, Clone, Serialize)]
pub struct Parsed {
    pub entities: Vec<Entity>,
    pub intent: Option<String>,
    pub intent_score: Option<f32>,
    pub count: i32,
}

pub struct Runtime {
    pub cfg: Config,
    tok: tokenizers::Tokenizer,
    emb: Embedding,
    mats: HashMap<String, QMatrix>,
    vecs: HashMap<String, Vec<f32>>,
    session: ort::session::Session,
    word_re: regex::Regex,
    _env: ort::environment::Environment,
}

struct Prompt {
    ids: Vec<u32>,
    word_first: Vec<usize>,
    markers: Vec<usize>,
    words: Vec<String>,
    offsets: Vec<(usize, usize)>,
}

/// Lê o `embedding.bin` int8 (VAELEMB1); reutilizado pelo runtime de decisão.
pub fn load_embedding(path: &Path) -> Result<Embedding> {
    let bytes = fs::read(path)?;
    if bytes.len() < 16 || &bytes[0..8] != b"VAELEMB1" {
        bail!("bad embedding header");
    }
    let rows = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
    let dim = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
    let n = rows
        .checked_mul(dim)
        .ok_or_else(|| anyhow!("embedding.bin: dimensões {rows}x{dim} inválidas"))?;
    let need = 16usize
        .checked_add(n)
        .and_then(|v| v.checked_add(rows.checked_mul(4)?))
        .ok_or_else(|| anyhow!("embedding.bin: dimensões {rows}x{dim} inválidas"))?;
    if bytes.len() < need {
        bail!(
            "embedding.bin truncado: {} bytes, esperava {need}",
            bytes.len()
        );
    }
    let q: Vec<i8> = bytes[16..16 + n].iter().map(|b| *b as i8).collect();
    let scales: Vec<f32> = bytes[16 + n..16 + n + rows * 4]
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes(c.try_into().unwrap()))
        .collect();
    Ok(Embedding { rows, dim, q, scale: scales })
}

/// Lê `n` bytes a partir de `off`, avançando o cursor; erro em vez de panic
/// quando o artefacto está truncado.
fn take<'a>(b: &'a [u8], off: &mut usize, n: usize) -> Result<&'a [u8]> {
    let start = *off;
    let end = start
        .checked_add(n)
        .ok_or_else(|| anyhow!("heads.bin: offset inválido"))?;
    let slice = b.get(start..end).ok_or_else(|| {
        let have = b.len().saturating_sub(start);
        anyhow!("heads.bin truncado em {start}: pedidos {n} bytes, há {have}")
    })?;
    *off = end;
    Ok(slice)
}

fn load_heads(path: &Path) -> Result<(HashMap<String, QMatrix>, HashMap<String, Vec<f32>>)> {
    let b = fs::read(path)?;
    if b.len() < 9 || &b[0..9] != b"VAELHEAD1" {
        bail!("bad heads header");
    }
    let mut off = 9usize;
    let mut mats = HashMap::new();
    let mut vecs = HashMap::new();
    while off + 2 <= b.len() {
        let nlen = u16::from_le_bytes(take(&b, &mut off, 2)?.try_into().unwrap()) as usize;
        let name = String::from_utf8_lossy(take(&b, &mut off, nlen)?).to_string();
        let dims = take(&b, &mut off, 8)?;
        let rows = u32::from_le_bytes(dims[0..4].try_into().unwrap()) as usize;
        let cols = u32::from_le_bytes(dims[4..8].try_into().unwrap()) as usize;
        let _bits = take(&b, &mut off, 1)?[0];
        let n = rows
            .checked_mul(cols)
            .ok_or_else(|| anyhow!("heads.bin: {name} com dimensões {rows}x{cols} inválidas"))?;
        let q: Vec<i8> = take(&b, &mut off, n)?.iter().map(|x| *x as i8).collect();
        let scale_bytes = rows
            .checked_mul(4)
            .ok_or_else(|| anyhow!("heads.bin: {name} com {rows} linhas inválidas"))?;
        let scale: Vec<f32> = take(&b, &mut off, scale_bytes)?
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes(c.try_into().unwrap()))
            .collect();
        mats.insert(name, QMatrix { rows, cols, q, scale });

        let vlen = u16::from_le_bytes(take(&b, &mut off, 2)?.try_into().unwrap()) as usize;
        let vname = String::from_utf8_lossy(take(&b, &mut off, vlen)?).to_string();
        let vn = u32::from_le_bytes(take(&b, &mut off, 4)?.try_into().unwrap()) as usize;
        let vbytes = vn
            .checked_mul(4)
            .ok_or_else(|| anyhow!("heads.bin: {vname} com {vn} valores inválidos"))?;
        let v: Vec<f32> = take(&b, &mut off, vbytes)?
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes(c.try_into().unwrap()))
            .collect();
        // Um vetor de comprimento 1 é um bias legítimo (`cls2.b`); só o
        // comprimento 0 é que não representa viés nenhum.
        if !v.is_empty() {
            vecs.insert(vname, v);
        }
    }
    Ok((mats, vecs))
}

fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b.iter()).map(|(x, y)| x * y).sum()
}

/// Confere as formas de `heads.bin` contra `nlu_config.json` no arranque.
/// Sem isto, um artefacto trocado só rebentava mais tarde, a meio do decode,
/// com slices fora de alcance.
fn validate_heads(
    cfg: &Config,
    mats: &HashMap<String, QMatrix>,
    vecs: &HashMap<String, Vec<f32>>,
) -> Result<()> {
    let h = cfg.hidden_size;
    let matrix = |name: &str, rows: usize, cols: usize| -> Result<()> {
        let m = mats
            .get(name)
            .ok_or_else(|| anyhow!("heads.bin sem a matriz '{name}'"))?;
        if m.rows != rows || m.cols != cols {
            bail!(
                "{name}: {}x{}, esperava {rows}x{cols}",
                m.rows,
                m.cols
            );
        }
        Ok(())
    };
    matrix("cp0", 2 * h, h)?;
    matrix("cp2", cfg.max_count, 2 * h)?;
    matrix("cls0", 2 * h, h)?;
    matrix("cls2", 1, 2 * h)?;
    matrix("gru_wih", 3 * h, h)?;
    matrix("gru_whh", 3 * h, h)?;
    matrix("pos", cfg.max_count, h)?;
    matrix("proj0", 4 * h, 2 * h)?;
    matrix("proj3", h, 4 * h)?;
    matrix("start0", 4 * h, h)?;
    matrix("start3", h, 4 * h)?;
    matrix("end0", 4 * h, h)?;
    matrix("end3", h, 4 * h)?;
    matrix("out0", 4 * h, 2 * h)?;
    matrix("out3", h, 4 * h)?;

    for (name, len) in [
        ("cp0.b", 2 * h),
        ("cp2.b", cfg.max_count),
        ("cls0.b", 2 * h),
        ("cls2.b", 1),
        ("gru_wih.b", 3 * h),
        ("gru_whh.b", 3 * h),
        ("proj0.b", 4 * h),
        ("proj3.b", h),
        ("start0.b", 4 * h),
        ("start3.b", h),
        ("end0.b", 4 * h),
        ("end3.b", h),
        ("out0.b", 4 * h),
        ("out3.b", h),
    ] {
        let v = vecs
            .get(name)
            .ok_or_else(|| anyhow!("heads.bin sem o vetor '{name}'"))?;
        if v.len() != len {
            bail!("{name} tem {} valores, esperava {len}", v.len());
        }
    }
    Ok(())
}

fn sigmoid(x: f32) -> f32 {
    1.0 / (1.0 + (-x).exp())
}

fn relu_inplace(x: &mut [f32]) {
    for v in x.iter_mut() {
        if *v < 0.0 {
            *v = 0.0;
        }
    }
}

// `(?i)` espelha o `re.IGNORECASE` do `WhitespaceTokenSplitter` de origem:
// sem ele, URLs/emails/@handles em maiúsculas são partidos em vários tokens e
// a sequência de subwords (e os spans) diverge da referência.
const WORD_CLASS: &str = concat!(
    r"(?i)(?:https?://[^\s]+|www\.[^\s]+)",
    r"|[a-z0-9._%+-]+@[a-z0-9.-]+\.[a-z]{2,}",
    r"|@[a-z0-9_]+",
    r"|[\w\u{0300}-\u{036f}\u{1ab0}-\u{1aff}\u{1dc0}-\u{1dff}\u{20d0}-\u{20ff}\u{fe20}-\u{fe2f}\u{200c}\u{200d}]+(?:[-_][\w\u{0300}-\u{036f}\u{1ab0}-\u{1aff}\u{1dc0}-\u{1dff}\u{20d0}-\u{20ff}\u{fe20}-\u{fe2f}\u{200c}\u{200d}]+)*",
    r"|\S",
);

impl Runtime {
    pub fn load(dir: impl AsRef<Path>) -> Result<Self> {
        let dir = dir.as_ref();
        let cfg: Config = serde_json::from_str(&fs::read_to_string(dir.join("nlu_config.json"))?)?;
        let tok = tokenizers::Tokenizer::from_file(dir.join("tokenizer.json"))
            .map_err(|e| anyhow!("tokenizer: {e}"))?;
        let emb = load_embedding(&dir.join("embedding.bin"))?;
        let (mats, vecs) = load_heads(&dir.join("heads.bin"))?;
        validate_heads(&cfg, &mats, &vecs)?;

        // Sem estes invariantes um id fora da tabela de embedding rebenta no
        // gather (index out of bounds) em vez de dar um erro de artefacto.
        if emb.rows != cfg.vocab_size {
            bail!(
                "embedding.bin tem {} linhas, nlu_config.vocab_size={}",
                emb.rows,
                cfg.vocab_size
            );
        }
        if emb.dim != cfg.hidden_size {
            bail!(
                "embedding.bin dim {} != nlu_config.hidden_size={}",
                emb.dim,
                cfg.hidden_size
            );
        }
        let tok_vocab = tok.get_vocab_size(true);
        if tok_vocab > emb.rows {
            bail!("tokenizer tem {tok_vocab} tokens, embedding só {} linhas", emb.rows);
        }
        if cfg.unk_id as usize >= emb.rows {
            bail!(
                "nlu_config.unk_id={} fora do embedding ({} linhas)",
                cfg.unk_id,
                emb.rows
            );
        }
        if let Some((name, id)) = cfg.specials.iter().find(|(_, id)| **id as usize >= emb.rows) {
            bail!(
                "special {name} com id {id} fora do embedding ({} linhas)",
                emb.rows
            );
        }

        let env = ort::init().with_name("vael-nlu").build().map_err(|e| anyhow!("{e:?}"))?;
        let mut builder = ort::session::Session::builder(&env).map_err(|e| anyhow!("{e:?}"))?;
        let mut builder = builder
            .with_optimization_level(ort::session::builder::GraphOptimizationLevel::All)
            .map_err(|e| anyhow!("{e:?}"))?;
        let threads = cfg.threads.max(1);
        let mut builder = builder
            .with_intra_threads(threads)
            .map_err(|e| anyhow!("{e:?}"))?;
        let mut builder = builder
            .with_inter_threads(1)
            .map_err(|e| anyhow!("{e:?}"))?;
        let session = builder
            .commit_from_file(dir.join("encoder.onnx"))
            .map_err(|e| anyhow!("{e:?}"))?;

        Ok(Self {
            cfg,
            tok,
            emb,
            mats,
            vecs,
            session,
            word_re: regex::Regex::new(WORD_CLASS)?,
            _env: env,
        })
    }

    fn tokenize_word(&self, w: &str) -> Vec<String> {
        self.tok
            .encode(w, false)
            .map(|e| e.get_tokens().to_vec())
            .unwrap_or_default()
    }

    fn id_of(&self, token: &str) -> u32 {
        if let Some(i) = self.cfg.specials.get(token) {
            return *i;
        }
        self.tok.token_to_id(token).unwrap_or(self.cfg.unk_id)
    }

    fn build_prompt(&self, text: &str, with_intent: bool) -> Prompt {
        let mut schemas: Vec<Vec<String>> = Vec::new();
        let mut ent = vec!["(".to_string(), "[P]".into(), "entities".into(), "(".into()];
        for l in &self.cfg.entity_labels {
            ent.push("[E]".into());
            ent.push(l.clone());
        }
        ent.push(")".into());
        ent.push(")".into());
        schemas.push(ent);
        if with_intent {
            let mut cls = vec!["(".to_string(), "[P]".into(), "intencao".into(), "(".into()];
            for l in &self.cfg.intent_labels {
                cls.push("[C]".into());
                cls.push(l.clone());
            }
            cls.push(")".into());
            cls.push(")".into());
            schemas.push(cls);
        }

        let mut combined: Vec<String> = Vec::new();
        for s in &schemas {
            combined.extend(s.iter().cloned());
            combined.push("[SEP_STRUCT]".into());
        }
        combined.pop();
        combined.push("[SEP_TEXT]".into());

        let mut words = Vec::new();
        let mut offsets = Vec::new();
        for m in self.word_re.find_iter(text) {
            words.push(m.as_str().to_lowercase());
            offsets.push((m.start(), m.end()));
        }

        let mut subwords: Vec<String> = Vec::new();
        let mut markers = Vec::new();
        for t in &combined {
            if t == "[P]" || t == "[E]" || t == "[C]" {
                markers.push(subwords.len());
            }
            if self.cfg.specials.contains_key(t) {
                subwords.push(t.clone());
            } else {
                subwords.extend(self.tokenize_word(t));
            }
        }
        let mut word_first = Vec::with_capacity(words.len());
        for w in &words {
            word_first.push(subwords.len());
            subwords.extend(self.tokenize_word(w));
        }

        let mut ids: Vec<u32> = subwords.iter().map(|t| self.id_of(t)).collect();
        if ids.len() > self.cfg.max_tokens {
            ids.truncate(self.cfg.max_tokens);
            // `markers` e `word_first` guardam índices no vetor de subwords
            // original. Sem os cortar também, um texto com mais de
            // `max_tokens` subwords deixa índices pendurados e o acesso a
            // `hidden` (encoder) rebenta com slice out of range.
            markers.retain(|&m| m < ids.len());
            let keep = word_first.iter().take_while(|&&wf| wf < ids.len()).count();
            word_first.truncate(keep);
            words.truncate(keep);
            offsets.truncate(keep);
        }
        Prompt { ids, word_first, markers, words, offsets }
    }

    pub fn parse(&mut self, text: &str) -> Result<Parsed> {
        self.parse_opts(text, true, None)
    }

    pub fn parse_opts(&mut self, text: &str, with_intent: bool, threshold: Option<f32>) -> Result<Parsed> {
        let p = self.build_prompt(text, with_intent);
        let n_ent = self.cfg.entity_labels.len();
        let h_dim = self.cfg.hidden_size;

        // Um prompt truncado ao ponto de perder o fim do schema não tem os
        // marcadores que as cabeças indexam. Devolver vazio é melhor do que
        // rebentar em `markers[..]`/`hidden[..]`.
        let need_markers = if with_intent { 2 + n_ent } else { 1 + n_ent };
        if p.markers.len() < need_markers {
            return Ok(Parsed {
                entities: Vec::new(),
                intent: None,
                intent_score: None,
                count: 0,
            });
        }

        let t_all = std::time::Instant::now();
        let mut t_pre = std::time::Duration::ZERO;
        let mut t_proj = std::time::Duration::ZERO;
        let mut t_fold = std::time::Duration::ZERO;
        let mut emb = Vec::new();
        self.emb.gather_into(&p.ids, &mut emb);
        let t_emb = t_all.elapsed();
        let n = p.ids.len();
        let emb_shape = [1usize, n, h_dim];
        let mask = vec![1i64; n];
        let mask_shape = [1usize, n];
        let outputs = self
            .session
            .run(ort::inputs![
                ort::value::TensorRef::from_array_view((emb_shape, &emb[..]))
                    .map_err(|e| anyhow!("{e:?}"))?,
                ort::value::TensorRef::from_array_view((mask_shape, &mask[..]))
                    .map_err(|e| anyhow!("{e:?}"))?
            ])
            .map_err(|e| anyhow!("{e:?}"))?;
        if outputs.len() != 1 {
            bail!("encoder devolveu {} saídas, esperava 1", outputs.len());
        }
        let (shape, data) = outputs[0]
            .try_extract_tensor::<f32>()
            .map_err(|e| anyhow!("{e:?}"))?;
        // O resto do decode assume batch 1 e [seq, hidden] contíguos; uma
        // forma diferente indexava `hidden` com offsets errados (lixo) em vez
        // de dar erro.
        if shape.len() != 3
            || shape[0] != 1
            || shape[1] != n as i64
            || shape[2] != h_dim as i64
        {
            bail!("encoder devolveu a forma {shape}, esperava [1, {n}, {h_dim}]");
        }
        let hidden = data.to_vec();
        drop(outputs);
        let t_enc = t_all.elapsed();

        // --- count head: count_pred([P]) ---
        let p_pos = p.markers[0];
        let mut cp = vec![0.0f32; 2 * h_dim];
        let m = |k: &str| self.mats.get(k).ok_or_else(|| anyhow!("missing {k}"));
        m("cp0")?.mv_into(&hidden[p_pos * h_dim..(p_pos + 1) * h_dim], self.vecs.get("cp0.b").map(|v| v.as_slice()), &mut cp);
        relu_inplace(&mut cp);
        let mut count_logits = vec![0.0f32; self.cfg.max_count];
        m("cp2")?.mv_into(&cp, self.vecs.get("cp2.b").map(|v| v.as_slice()), &mut count_logits);
        // `argmax` da referência devolve o PRIMEIRO máximo; `max_by` devolve o
        // último em caso de empate. Um fold explícito com `>` mantém a paridade
        // (e não rebenta se um logit for NaN).
        let mut predicted = 0i32;
        let mut best_logit = f32::NEG_INFINITY;
        for (i, logit) in count_logits.iter().enumerate() {
            if *logit > best_logit {
                best_logit = *logit;
                predicted = i as i32;
            }
        }
        // The count head is only a gate for the first structural query. Bare
        // nouns ("nota", "poço") sometimes predict 0 and would drop every
        // span; with any text present, one structural instance always exists.
        let count = if predicted <= 0 && !text.trim().is_empty() { 1 } else { predicted };

        let mut entities = Vec::new();
        if count > 0 {
            // --- count_embed: GRU over the label embeddings, step 0 ---
            let field_positions: Vec<usize> = p.markers[1..1 + n_ent].to_vec();
            let mut fields = vec![0.0f32; n_ent * h_dim];
            for (i, pos) in field_positions.iter().enumerate() {
                fields[i * h_dim..(i + 1) * h_dim]
                    .copy_from_slice(&hidden[pos * h_dim..(pos + 1) * h_dim]);
            }
            let dim3 = 3 * h_dim;
            let mut gi = vec![0.0f32; dim3];
            let pos0 = &self.mats.get("pos").ok_or_else(|| anyhow!("missing pos"))?.q[0..h_dim];
            let pos_scale = self.mats.get("pos").unwrap().scale[0];
            let pos_vec: Vec<f32> = pos0.iter().map(|q| *q as f32 * pos_scale).collect();
            self.mats
                .get("gru_wih")
                .ok_or_else(|| anyhow!("missing gru_wih"))?
                .mv_into(&pos_vec, self.vecs.get("gru_wih.b").map(|v| v.as_slice()), &mut gi);
            let mut gh = vec![0.0f32; n_ent * dim3];
            self.mats
                .get("gru_whh")
                .ok_or_else(|| anyhow!("missing gru_whh"))?
                .mm_into(&fields, n_ent, self.vecs.get("gru_whh.b").map(|v| v.as_slice()), &mut gh);

            let mut cat = vec![0.0f32; n_ent * 2 * h_dim];
            for f in 0..n_ent {
                let mut h = vec![0.0f32; h_dim];
                for d in 0..h_dim {
                    let ir = gi[d];
                    let iz = gi[h_dim + d];
                    let inn = gi[2 * h_dim + d];
                    let hr = gh[f * dim3 + d];
                    let hz = gh[f * dim3 + h_dim + d];
                    let hn = gh[f * dim3 + 2 * h_dim + d];
                    let r = sigmoid(ir + hr);
                    let z = sigmoid(iz + hz);
                    let nn = (inn + r * hn).tanh();
                    h[d] = (1.0 - z) * nn + z * fields[f * h_dim + d];
                }
                cat[f * 2 * h_dim..f * 2 * h_dim + h_dim].copy_from_slice(&h);
                cat[f * 2 * h_dim + h_dim..(f + 1) * 2 * h_dim]
                    .copy_from_slice(&fields[f * h_dim..(f + 1) * h_dim]);
            }
            let mut proj = vec![0.0f32; n_ent * 4 * h_dim];
            self.mats
                .get("proj0")
                .ok_or_else(|| anyhow!("missing proj0"))?
                .mm4_into(&cat, n_ent, self.vecs.get("proj0.b").map(|v| v.as_slice()), &mut proj);
            relu_inplace(&mut proj);
            let mut queries = vec![0.0f32; n_ent * h_dim];
            self.mats
                .get("proj3")
                .ok_or_else(|| anyhow!("missing proj3"))?
                .mm4_into(&proj, n_ent, self.vecs.get("proj3.b").map(|v| v.as_slice()), &mut queries);

            // --- span head ---
            let lw = p.word_first.len();
            let mut word_h = vec![0.0f32; lw * h_dim];
            for (i, wf) in p.word_first.iter().enumerate() {
                word_h[i * h_dim..(i + 1) * h_dim]
                    .copy_from_slice(&hidden[wf * h_dim..(wf + 1) * h_dim]);
            }
            t_pre = t_all.elapsed();
            let (start_h, end_h) = self.project_spans(&word_h, lw)?;
            t_proj = t_all.elapsed();
            let out0 = self.mats.get("out0").ok_or_else(|| anyhow!("missing out0"))?;
            let out3 = self.mats.get("out3").ok_or_else(|| anyhow!("missing out3"))?;
            let hidden4 = out0.rows; // 4H
            let out_b = self.vecs.get("out0.b").ok_or_else(|| anyhow!("missing out0.b"))?;
            let out3_b = self.vecs.get("out3.b").ok_or_else(|| anyhow!("missing out3.b"))?;
            if out_b.len() != hidden4 {
                bail!("out0.b tem {} valores, esperava {hidden4}", out_b.len());
            }
            if out3_b.len() != h_dim {
                bail!("out3.b tem {} valores, esperava {h_dim}", out3_b.len());
            }

            let mut folded = vec![0.0f32; n_ent * hidden4];
            let mut qbias = vec![0.0f32; n_ent];
            for q in 0..n_ent {
                // logits = hidden · (W2^T q) + q·b2  with W2 stored [H, 4H]
                let qv = &queries[q * h_dim..(q + 1) * h_dim];
                out3.gemv_transposed(qv, &mut folded[q * hidden4..(q + 1) * hidden4]);
                qbias[q] = dot(qv, out3_b);
            }

            t_fold = t_all.elapsed();
            let th = threshold.unwrap_or(self.cfg.threshold);
            let width = self.cfg.max_width;
            let mut per_label: Vec<Vec<(f32, usize, usize)>> = vec![Vec::new(); n_ent];
            for q in 0..n_ent {
                for s in 0..lw {
                    for w in 0..width {
                        let e = s + w;
                        if e >= lw {
                            break;
                        }
                        let mut acc = 0.0f32;
                        for o in 0..hidden4 {
                            let v = start_h[s * hidden4 + o] + end_h[e * hidden4 + o] + out_b[o];
                            if v > 0.0 {
                                acc += v * folded[q * hidden4 + o];
                            }
                        }
                        let score = sigmoid(acc + qbias[q]);
                        if score >= th {
                            per_label[q].push((score, s, e));
                        }
                    }
                }
            }
            for (q, cands) in per_label.iter_mut().enumerate() {
                // A referência ordena tuplas com `reverse=True`: score, depois
                // `s`, depois `e`, todos decrescentes. Desempatar só pelo score
                // escolhia o span mais cedo/curto em vez do mais tarde/longo.
                cands.sort_by(|a, b| {
                    b.0.partial_cmp(&a.0)
                        .unwrap_or(std::cmp::Ordering::Equal)
                        .then_with(|| b.1.cmp(&a.1))
                        .then_with(|| b.2.cmp(&a.2))
                });
                let mut taken: Vec<(usize, usize)> = Vec::new();
                for (sc, s, e) in cands.iter() {
                    if taken.iter().any(|(ts, te)| !(*e < *ts || *s > *te)) {
                        continue;
                    }
                    taken.push((*s, *e));
                    let (bs, be) = (p.offsets[*s].0, p.offsets[*e].1);
                    // The splitter yields byte offsets; report character
                    // offsets so Lua/UTF-8 consumers agree with the reference.
                    let cs = text[..bs].chars().count();
                    let ce = text[..be].chars().count();
                    entities.push(Entity {
                        label: self
                            .cfg
                            .entity_map
                            .get(&self.cfg.entity_labels[q])
                            .cloned()
                            .unwrap_or_else(|| self.cfg.entity_labels[q].clone()),
                        start: cs,
                        end: ce,
                        text: text[bs..be].to_string(),
                        score: *sc,
                    });
                }
            }
        }

        // --- intent classifier ---
        let mut intent = None;
        let mut intent_score = None;
        if with_intent {
            let cls_start = 1 + n_ent; // [P] of the second schema
            let label_positions = &p.markers[cls_start + 1..];
            let cls0 = self.mats.get("cls0").ok_or_else(|| anyhow!("missing cls0"))?;
            let cls2 = self.mats.get("cls2").ok_or_else(|| anyhow!("missing cls2"))?;
            let mut best = (0usize, -1.0f32);
            for (i, pos) in label_positions.iter().enumerate() {
                let mut mid = vec![0.0f32; 2 * h_dim];
                cls0.mv_into(&hidden[pos * h_dim..(pos + 1) * h_dim], self.vecs.get("cls0.b").map(|v| v.as_slice()), &mut mid);
                relu_inplace(&mut mid);
                let mut logit = vec![0.0f32; cls2.rows];
                cls2.mv_into(&mid, self.vecs.get("cls2.b").map(|v| v.as_slice()), &mut logit);
                let p = sigmoid(logit[0]);
                if p > best.1 {
                    best = (i, p);
                }
            }
            if best.0 < self.cfg.intent_labels.len()
                && best.1 >= self.cfg.intent_threshold
            {
                intent = Some(self.cfg.intent_labels[best.0].clone());
                intent_score = Some(best.1);
            }
        }

        if std::env::var("VAEL_NLU_TIMING").is_ok() {
            eprintln!(
                "emb {:?} enc {:?} pre {:?} proj {:?} fold {:?} total {:?}",
                t_emb, t_enc, t_pre.saturating_sub(t_enc), t_proj.saturating_sub(t_pre),
                t_fold.saturating_sub(t_proj), t_all.elapsed()
            );
        }
        Ok(Parsed { entities, intent, intent_score, count })
    }

    fn project_spans(&self, word_h: &[f32], lw: usize) -> Result<(Vec<f32>, Vec<f32>)> {
        let h = self.cfg.hidden_size;
        let s0 = self.mats.get("start0").ok_or_else(|| anyhow!("missing start0"))?;
        let s3 = self.mats.get("start3").ok_or_else(|| anyhow!("missing start3"))?;
        let e0 = self.mats.get("end0").ok_or_else(|| anyhow!("missing end0"))?;
        let e3 = self.mats.get("end3").ok_or_else(|| anyhow!("missing end3"))?;
        let out0 = self.mats.get("out0").ok_or_else(|| anyhow!("missing out0"))?;
        let hidden4 = out0.rows; // 4H, a largura real da projeção fundida
        if s0.cols != h
            || s0.rows != hidden4
            || s3.cols != hidden4
            || s3.rows != h
            || e0.cols != h
            || e0.rows != hidden4
            || e3.cols != hidden4
            || e3.rows != h
            || out0.cols < 2 * h
        {
            bail!("span head com formas inesperadas para hidden_size={h}");
        }

        // Batched 2H -> 4H -> H projection for every word.
        let mut tmp = vec![0.0f32; lw * hidden4];
        s0.mm4_into(word_h, lw, self.vecs.get("start0.b").map(|v| v.as_slice()), &mut tmp);
        relu_inplace(&mut tmp);
        let mut rep = vec![0.0f32; lw * h];
        s3.mm4_into(&tmp, lw, self.vecs.get("start3.b").map(|v| v.as_slice()), &mut rep);
        relu_inplace(&mut rep);
        let mut start_h = vec![0.0f32; lw * hidden4];
        out0.mm4_cols_into(&rep, lw, 0, h, &mut start_h);

        e0.mm4_into(word_h, lw, self.vecs.get("end0.b").map(|v| v.as_slice()), &mut tmp);
        relu_inplace(&mut tmp);
        e3.mm4_into(&tmp, lw, self.vecs.get("end3.b").map(|v| v.as_slice()), &mut rep);
        relu_inplace(&mut rep);
        let mut end_h = vec![0.0f32; lw * hidden4];
        out0.mm4_cols_into(&rep, lw, h, h, &mut end_h);

        Ok((start_h, end_h))
    }
}

/// Public helper used by tests and by the game integration to debug offsets.
pub fn split_words(text: &str) -> Vec<(String, usize, usize)> {
    let re = regex::Regex::new(WORD_CLASS).unwrap();
    re.find_iter(text)
        .map(|m| (m.as_str().to_lowercase(), m.start(), m.end()))
        .collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn splitter_matches_offsets() {
        let t = "Quero comprar duas poções de cura do mercador.";
        let w = super::split_words(t);
        for (s, a, b) in &w {
            assert_eq!(t[*a..*b].to_lowercase(), *s, "{s} {a} {b}");
        }
        let (_, bs, be) = w[7];
        assert_eq!(t[..bs].chars().count(), 37);
        assert_eq!(t[..be].chars().count(), 45);
    }

    #[test]
    fn splitter_is_case_insensitive_like_the_reference() {
        // `WhitespaceTokenSplitter` compila com re.IGNORECASE: URL/email/@handle
        // em maiúsculas são UM token, não uma sequência de pedaços.
        let words: Vec<String> = super::split_words("WWW.EXAMPLE.COM Fulano@Example.COM @Ana")
            .into_iter()
            .map(|(w, _, _)| w)
            .collect();
        assert_eq!(words, ["www.example.com", "fulano@example.com", "@ana"]);
    }

    fn tmp_path(tag: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "vael-nlu-{}-{tag}-{nanos}",
            std::process::id()
        ))
    }

    #[test]
    fn load_heads_keeps_single_value_vectors() {
        let mut b = Vec::new();
        b.extend_from_slice(b"VAELHEAD1");
        let name = b"m";
        b.extend_from_slice(&(name.len() as u16).to_le_bytes());
        b.extend_from_slice(name);
        b.extend_from_slice(&1u32.to_le_bytes()); // rows
        b.extend_from_slice(&2u32.to_le_bytes()); // cols
        b.push(4); // bits
        b.extend_from_slice(&[1u8, 254u8]); // 1 e -2 em i8
        b.extend_from_slice(&0.5f32.to_le_bytes()); // scale
        let vname = b"m.b";
        b.extend_from_slice(&(vname.len() as u16).to_le_bytes());
        b.extend_from_slice(vname);
        b.extend_from_slice(&1u32.to_le_bytes()); // um único bias
        b.extend_from_slice(&0.25f32.to_le_bytes());

        let path = tmp_path("heads-singleton");
        std::fs::write(&path, &b).unwrap();
        let loaded = super::load_heads(&path);
        let _ = std::fs::remove_file(&path);
        let (_mats, vecs) = loaded.unwrap();
        assert_eq!(
            vecs.get("m.b").map(|v| v.as_slice()),
            Some(&[0.25f32][..])
        );
    }

    #[test]
    fn truncated_artifacts_are_errors_not_panics() {
        let path = tmp_path("emb-short");
        std::fs::write(&path, b"VAEL").unwrap();
        assert!(super::load_embedding(&path).is_err());
        let _ = std::fs::remove_file(&path);

        // Cabeçalho válido, nome com 5 bytes declarados mas só 2 presentes.
        let path = tmp_path("heads-short");
        std::fs::write(&path, b"VAELHEAD1\x05\x00ab").unwrap();
        assert!(super::load_heads(&path).is_err());
        let _ = std::fs::remove_file(&path);
    }

    /// Integração real, gated: `VAEL_NLU_DIR=../Vael/assets/public/nlu cargo test`.
    #[test]
    fn long_text_is_truncated_instead_of_panicking() {
        let Ok(dir) = std::env::var("VAEL_NLU_DIR") else {
            return;
        };
        let mut rt = super::Runtime::load(dir).expect("carregar artefacto");
        let text = "lobos ".repeat(2000);
        let parsed = rt.parse(&text).expect("parse de texto longo");
        let n_chars = text.chars().count();
        assert!(parsed.entities.iter().all(|e| e.end <= n_chars));
        assert!(parsed.entities.iter().all(|e| !e.text.is_empty()));
    }
}
