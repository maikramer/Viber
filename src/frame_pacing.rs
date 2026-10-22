//! Frame pacing — frames com a MESMA duração em vez de uma média boa.
//!
//! Com vsync (FIFO) a apresentação só acontece num vblank: a 144 Hz um frame
//! de 22–26 ms cai ora em 4 vblanks (27,8 ms), ora em 3 (20,8), ora em 5
//! (34,7). A média até é decente, mas o herói parado treme — é a alternância
//! que se vê, não o fps. O pacer dorme no fim de cada frame até um período
//! que é um DIVISOR do refresh (144 → 72 / 48 / 36 / 28,8 fps), para que todos
//! os frames ocupem o mesmo número de vblanks.
//!
//! `VIBER_FPS_CAP`:
//! * `auto` (omissão) — escolhe o divisor mais rápido que o jogo sustenta
//!   (p90 do trabalho por frame com 8 % de folga). Desce logo que falha,
//!   sobe só depois de [`UPSHIFT_SECONDS`] seguidos com folga.
//! * `N` — alvo fixo em fps (não é arredondado ao refresh).
//! * `0` / `off` — sem pacing (o comportamento antigo).

use std::time::{Duration, Instant};

use bevy::prelude::*;
use bevy::window::{Monitor, PrimaryMonitor};

/// Refresh assumido quando o monitor não o reporta.
const FALLBACK_REFRESH_HZ: f32 = 60.0;
/// Folga sobre o p90 do trabalho ao escolher o período.
const HEADROOM: f32 = 1.08;
/// Segundos seguidos com folga antes de subir de divisor.
pub const UPSHIFT_SECONDS: u32 = 3;
/// Frames na janela do p90 (1 s a 60 fps). Curta de propósito: a primeira
/// versão (180) guardava os frames do arranque e prendia o pacer em 21 fps
/// durante ~8 s.
const WINDOW: usize = 60;
/// Frames acima disto são stalls (carga, troca de cena), não carga de
/// regime: não decidem o período.
const STALL: f32 = 0.1;
/// O auto nunca passa disto, nem que o monitor seja de 240 Hz.
const AUTO_MAX_FPS: f32 = 72.0;
/// Abaixo disto o auto larga o pacing: segurar 21 fps no arranque (frames
/// de carga de 50–100 ms) criava picos de 50 ms autoinfligidos.
const AUTO_MIN_FPS: f32 = 28.0;
/// Margem final em espera ativa: o `sleep` do SO acorda tarde ~1 ms.
const SPIN: Duration = Duration::from_micros(1500);

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PacingMode {
    Off,
    Fixed(f32),
    Auto,
}

impl PacingMode {
    pub fn parse(raw: Option<&str>) -> Self {
        match raw.map(|r| r.trim().to_ascii_lowercase()) {
            None => Self::Auto,
            Some(r) if r.is_empty() || r == "auto" => Self::Auto,
            Some(r) if r == "0" || r == "off" || r == "false" => Self::Off,
            Some(r) => match r.parse::<f32>() {
                Ok(fps) if fps.is_finite() && fps > 0.0 => Self::Fixed(fps),
                _ => Self::Auto,
            },
        }
    }
}

/// Divisores do refresh, do mais rápido para o mais lento, em períodos (s).
pub fn divisor_periods(refresh_hz: f32) -> Vec<f32> {
    (1..=8)
        .map(|k| refresh_hz / k as f32)
        .filter(|fps| *fps <= AUTO_MAX_FPS + 0.01 && *fps >= AUTO_MIN_FPS)
        .map(|fps| 1.0 / fps)
        .collect()
}

/// O período mais curto da lista que cobre `work` (s) com folga; `None`
/// quando nenhum cobre (sem pacing — melhor do que um período longo).
pub fn pick_period(periods: &[f32], work: f32) -> Option<f32> {
    periods.iter().copied().find(|p| *p >= work * HEADROOM)
}

#[derive(Resource)]
pub struct FramePacer {
    pub mode: PacingMode,
    /// Período em vigor (s); `None` = sem pacing.
    pub period: Option<f32>,
    /// Trabalho do último frame (s), SEM o sono do pacer — o que o
    /// `adaptive_quality` deve medir (o frame real fica preso ao período).
    pub last_work: f32,
    mark: Option<Instant>,
    work: Vec<f32>,
    cursor: usize,
    since_eval: f32,
    upshift_streak: u32,
}

impl FramePacer {
    fn new(mode: PacingMode) -> Self {
        Self {
            mode,
            period: match mode {
                PacingMode::Fixed(fps) => Some(1.0 / fps),
                _ => None,
            },
            last_work: 0.0,
            mark: None,
            work: Vec::with_capacity(WINDOW),
            cursor: 0,
            since_eval: 0.0,
            upshift_streak: 0,
        }
    }

    fn record(&mut self, work: f32) {
        if work > STALL {
            return;
        }
        if self.work.len() < WINDOW {
            self.work.push(work);
        } else {
            self.work[self.cursor] = work;
            self.cursor = (self.cursor + 1) % WINDOW;
        }
    }

    fn p90(&self) -> Option<f32> {
        if self.work.len() < 30 {
            return None;
        }
        let mut sorted = self.work.clone();
        sorted.sort_by(f32::total_cmp);
        Some(sorted[(sorted.len() * 9) / 10])
    }

    /// Reavalia o período do `auto` (1× por segundo).
    fn evaluate(&mut self, refresh_hz: f32) {
        let Some(p90) = self.p90() else {
            return;
        };
        let periods = divisor_periods(refresh_hz);
        let Some(wanted) = pick_period(&periods, p90) else {
            self.period = None;
            self.upshift_streak = 0;
            return;
        };
        match self.period {
            // Primeira decisão, ou a falhar o período: muda já.
            None => self.period = Some(wanted),
            Some(current) if wanted > current + 1e-5 => {
                self.period = Some(wanted);
                self.upshift_streak = 0;
            }
            Some(current) if wanted < current - 1e-5 => {
                self.upshift_streak += 1;
                if self.upshift_streak >= UPSHIFT_SECONDS {
                    self.period = Some(wanted);
                    self.upshift_streak = 0;
                }
            }
            Some(_) => self.upshift_streak = 0,
        }
    }
}

pub struct FramePacingPlugin;

impl Plugin for FramePacingPlugin {
    fn build(&self, app: &mut App) {
        let mode = PacingMode::parse(std::env::var("VIBER_FPS_CAP").ok().as_deref());
        if mode == PacingMode::Off {
            return;
        }
        info!("frame-pacing: {mode:?}");
        app.insert_resource(FramePacer::new(mode))
            .add_systems(Last, pace_frame);
    }
}

fn pace_frame(
    mut pacer: ResMut<FramePacer>,
    primary: Query<&Monitor, With<PrimaryMonitor>>,
    monitors: Query<&Monitor>,
    mut last_period: Local<Option<f32>>,
) {
    let now = Instant::now();
    let Some(mark) = pacer.mark else {
        pacer.mark = Some(now);
        return;
    };
    let work = now.duration_since(mark).as_secs_f32();
    pacer.last_work = work;
    pacer.record(work);
    if pacer.mode == PacingMode::Auto {
        pacer.since_eval += work;
        if pacer.since_eval >= 1.0 {
            pacer.since_eval = 0.0;
            // Wayland não tem monitor "primário": sem ele, o refresh do
            // primeiro monitor que o reporte (cair nos 60 Hz de fallback num
            // ecrã de 144 dava um período que não divide o refresh e a
            // apresentação voltava a alternar entre 2 e 3 vblanks).
            let refresh = primary
                .iter()
                .chain(monitors.iter())
                .find_map(|m| m.refresh_rate_millihertz)
                .map_or(FALLBACK_REFRESH_HZ, |mhz| mhz as f32 / 1000.0);
            pacer.evaluate(refresh);
        }
    }
    if pacer.period != *last_period {
        *last_period = pacer.period;
        if pacer.period.is_none() {
            info!("frame-pacing: sem pacing (trabalho acima do divisor mais lento)");
        }
        if let Some(p) = pacer.period {
            let refresh = primary
                .iter()
                .chain(monitors.iter())
                .find_map(|m| m.refresh_rate_millihertz);
            info!(
                "frame-pacing: período {:.1} ms ({:.0} fps; refresh {:?} mHz)",
                p * 1000.0,
                1.0 / p,
                refresh
            );
        }
    }
    if let Some(period) = pacer.period {
        let deadline = mark + Duration::from_secs_f32(period);
        if let Some(left) = deadline.checked_duration_since(Instant::now()) {
            if left > SPIN {
                std::thread::sleep(left - SPIN);
            }
            while Instant::now() < deadline {
                std::hint::spin_loop();
            }
            // Marca no deadline (não no "agora"): o atraso de um frame não
            // empurra os seguintes.
            pacer.mark = Some(deadline);
            return;
        }
    }
    pacer.mark = Some(Instant::now());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_modes() {
        assert_eq!(PacingMode::parse(None), PacingMode::Auto);
        assert_eq!(PacingMode::parse(Some("off")), PacingMode::Off);
        assert_eq!(PacingMode::parse(Some("0")), PacingMode::Off);
        assert_eq!(PacingMode::parse(Some("48")), PacingMode::Fixed(48.0));
        assert_eq!(PacingMode::parse(Some("lixo")), PacingMode::Auto);
    }

    #[test]
    fn divisors_of_144_stay_under_the_auto_ceiling() {
        let fps: Vec<f32> = divisor_periods(144.0).iter().map(|p| 1.0 / p).collect();
        assert!((fps[0] - 72.0).abs() < 0.01, "{fps:?}");
        assert!((fps[1] - 48.0).abs() < 0.01);
        assert!((fps[2] - 36.0).abs() < 0.01);
    }

    #[test]
    fn picks_the_fastest_period_that_covers_the_work() {
        let periods = divisor_periods(144.0);
        // 18 ms de trabalho (×1,08 = 19,4) cabe nos 20,8 ms dos 48 fps.
        let p = pick_period(&periods, 0.018).unwrap();
        assert!((1.0 / p - 48.0).abs() < 0.01);
        // 24 ms não cabe em 20,8 → 27,8 ms (36 fps).
        let p = pick_period(&periods, 0.024).unwrap();
        assert!((1.0 / p - 36.0).abs() < 0.01);
    }

    /// Trabalho que não cabe em ≥ 28 fps: sem pacing, nunca 21 fps.
    #[test]
    fn heavy_work_turns_pacing_off() {
        let mut pacer = FramePacer::new(PacingMode::Auto);
        for _ in 0..WINDOW {
            pacer.record(0.06);
        }
        pacer.evaluate(144.0);
        assert_eq!(pacer.period, None);
    }

    /// Um stall de carga (> 100 ms) não empurra o período para os 21 fps.
    #[test]
    fn stalls_do_not_drive_the_period() {
        let mut pacer = FramePacer::new(PacingMode::Auto);
        for _ in 0..10 {
            pacer.record(0.4);
        }
        for _ in 0..WINDOW {
            pacer.record(0.012);
        }
        pacer.evaluate(144.0);
        assert!((1.0 / pacer.period.unwrap() - 72.0).abs() < 0.01);
    }

    #[test]
    fn auto_drops_at_once_and_climbs_only_after_a_streak() {
        let mut pacer = FramePacer::new(PacingMode::Auto);
        for _ in 0..WINDOW {
            pacer.record(0.024);
        }
        pacer.evaluate(144.0);
        assert!((1.0 / pacer.period.unwrap() - 36.0).abs() < 0.01);
        for _ in 0..WINDOW {
            pacer.record(0.015);
        }
        for _ in 0..UPSHIFT_SECONDS - 1 {
            pacer.evaluate(144.0);
            assert!((1.0 / pacer.period.unwrap() - 36.0).abs() < 0.01);
        }
        pacer.evaluate(144.0);
        assert!((1.0 / pacer.period.unwrap() - 48.0).abs() < 0.01);
        for _ in 0..WINDOW {
            pacer.record(0.030);
        }
        pacer.evaluate(144.0);
        assert!((1.0 / pacer.period.unwrap() - 28.8).abs() < 0.05);
    }
}
