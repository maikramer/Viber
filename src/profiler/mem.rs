//! Memória do processo para o profiler — RSS, pico e nº de threads.
//!
//! A `wgpu` não expõe VRAM de forma portátil: o que se mede aqui é o
//! processo. No Linux os números vêm de `/proc/self/status` (`VmRSS`,
//! `VmHWM`, `Threads`); noutras plataformas os campos vêm `null` e o resto do
//! snapshot não muda de forma.
//!
//! A leitura custa dezenas de microssegundos — entra no snapshot a 4 Hz sem
//! mexer no frame. O pico (`VmHWM`) é o que interessa numa fuga de memória:
//! o RSS actual sobe e desce com o allocator, o pico nunca desce.

use serde::Serialize;

/// Memória do processo (MiB) + threads. `None` quando a plataforma não expõe
/// a leitura (ou o ficheiro não existe).
#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct MemorySnapshot {
    pub rss_mb: Option<f64>,
    pub peak_rss_mb: Option<f64>,
    pub threads: Option<u64>,
}

impl MemorySnapshot {
    /// Uma linha para o overlay F3 / CLI; `None` sem leitura.
    pub fn line(&self) -> Option<String> {
        match (self.rss_mb, self.peak_rss_mb) {
            (Some(rss), Some(peak)) => Some(match self.threads {
                Some(threads) => format!("{rss:.0} MiB (pico {peak:.0}) · {threads} thr"),
                None => format!("{rss:.0} MiB (pico {peak:.0})"),
            }),
            (Some(rss), None) => Some(format!("{rss:.0} MiB")),
            _ => None,
        }
    }
}

/// Lê a memória do processo. Barato; chamar à vontade.
pub fn snapshot() -> MemorySnapshot {
    #[cfg(target_os = "linux")]
    if let Ok(text) = std::fs::read_to_string("/proc/self/status") {
        return parse_status(&text);
    }
    MemorySnapshot::default()
}

/// Parser isolado do `/proc/self/status` (testável sem tocar no processo).
fn parse_status(text: &str) -> MemorySnapshot {
    let mut out = MemorySnapshot::default();
    for line in text.lines() {
        let Some((key, rest)) = line.split_once(':') else {
            continue;
        };
        let mut fields = rest.split_whitespace();
        match key {
            "VmRSS" => out.rss_mb = fields.next().and_then(|v| kb_to_mb(v)),
            "VmHWM" => out.peak_rss_mb = fields.next().and_then(|v| kb_to_mb(v)),
            "Threads" => out.threads = fields.next().and_then(|v| v.parse().ok()),
            _ => {}
        }
    }
    out
}

fn kb_to_mb(raw: &str) -> Option<f64> {
    raw.parse::<f64>().ok().map(|kb| kb / 1024.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_linux_status_fields() {
        let sample = "\
Name:\tviber
State:\tS (sleeping)
VmPeak:\t 2048000 kB
VmSize:\t 1900000 kB
VmHWM:\t  524288 kB
VmRSS:\t  393216 kB
RssAnon:\t  300000 kB
Threads:\t23
";
        let snap = parse_status(sample);
        assert!((snap.rss_mb.unwrap() - 384.0).abs() < 1e-6);
        assert!((snap.peak_rss_mb.unwrap() - 512.0).abs() < 1e-6);
        assert_eq!(snap.threads, Some(23));
        assert_eq!(
            snap.line().as_deref(),
            Some("384 MiB (pico 512) · 23 thr")
        );
    }

    #[test]
    fn missing_fields_are_none() {
        let snap = parse_status("Name:\tviber\nState:\tS\n");
        assert_eq!(snap.rss_mb, None);
        assert_eq!(snap.peak_rss_mb, None);
        assert_eq!(snap.threads, None);
        assert_eq!(snap.line(), None);
    }
}
