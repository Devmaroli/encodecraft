//! Frame-time guard. Enable with `ENCODECRAFT_PERF=1`.

use std::time::{Duration, Instant};

use egui::{Color32, RichText};

pub fn enabled() -> bool {
    std::env::var("ENCODECRAFT_PERF").ok().as_deref() == Some("1")
}

pub struct PerfHud {
    pub enabled: bool,
    last: Instant,
    samples: Vec<f32>,
    over_16: u32,
    frames: u32,
    pub last_ms: f32,
    max_ms: f32,
    slow_ops: Vec<String>,
}

impl Default for PerfHud {
    fn default() -> Self {
        Self {
            enabled: enabled(),
            last: Instant::now(),
            samples: Vec::with_capacity(120),
            over_16: 0,
            frames: 0,
            last_ms: 0.0,
            max_ms: 0.0,
            slow_ops: Vec::new(),
        }
    }
}

impl PerfHud {
    pub fn begin_frame(&mut self) {
        self.last = Instant::now();
    }

    pub fn end_frame(&mut self) {
        let ms = self.last.elapsed().as_secs_f32() * 1000.0;
        self.last_ms = ms;
        self.frames = self.frames.saturating_add(1);
        if ms > self.max_ms {
            self.max_ms = ms;
        }
        if ms > 16.0 {
            self.over_16 = self.over_16.saturating_add(1);
            log::warn!("frame {ms:.1} ms (>16)");
        }
        self.samples.push(ms);
        if self.samples.len() > 90 {
            self.samples.remove(0);
        }
    }

    pub fn push_slow(&mut self, name: &str, ms: f64) {
        log::warn!("slow {name}: {ms:.1} ms");
        self.slow_ops.push(format!("{name} {ms:.0}ms"));
        if self.slow_ops.len() > 6 {
            self.slow_ops.remove(0);
        }
    }

    pub fn avg_ms(&self) -> f32 {
        if self.samples.is_empty() {
            return 0.0;
        }
        self.samples.iter().sum::<f32>() / self.samples.len() as f32
    }

    pub fn show(&self, ctx: &egui::Context) {
        if !self.enabled {
            return;
        }
        egui::Area::new(egui::Id::new("perf-hud")).fixed_pos(egui::pos2(8.0, 8.0)).show(ctx, |ui| {
            egui::Frame::NONE.fill(Color32::from_rgba_unmultiplied(0, 0, 0, 180)).inner_margin(6.0).show(ui, |ui| {
                ui.label(
                    RichText::new(format!(
                        "frame {last:.1} ms  avg {avg:.1}  max {max:.1}  >16ms {over}",
                        last = self.last_ms,
                        avg = self.avg_ms(),
                        max = self.max_ms,
                        over = self.over_16
                    ))
                    .color(Color32::from_rgb(0x9e, 0xdc, 0xff))
                    .size(11.0),
                );
                for s in &self.slow_ops {
                    ui.label(RichText::new(s).color(Color32::from_rgb(0xf0, 0xc0, 0x80)).size(11.0));
                }
            });
        });
    }
}

pub struct BenchRun {
    pub rows: usize,
    pub remaining: u32,
    pub warmup: u32,
    pub times: Vec<f32>,
    pub encode_two: bool,
}

impl BenchRun {
    pub fn from_env() -> Option<Self> {
        let raw = std::env::var("ENCODECRAFT_BENCH").ok()?;
        if raw != "1" && raw != "encode2" {
            return None;
        }
        let rows = std::env::var("ENCODECRAFT_BENCH_ROWS").ok().and_then(|s| s.parse().ok()).unwrap_or(1000);
        let frames = std::env::var("ENCODECRAFT_BENCH_FRAMES").ok().and_then(|s| s.parse().ok()).unwrap_or(90);
        Some(Self { rows, remaining: frames, warmup: 10, times: Vec::new(), encode_two: raw == "encode2" })
    }

    pub fn record(&mut self, dt: Duration) {
        if self.warmup > 0 {
            self.warmup = self.warmup.saturating_sub(1);
            return;
        }
        self.times.push(dt.as_secs_f32() * 1000.0);
        self.remaining = self.remaining.saturating_sub(1);
    }

    pub fn report_and_exit(&self) {
        let mut s = self.times.clone();
        s.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let n = s.len().max(1);
        let avg = s.iter().sum::<f32>() / n as f32;
        let p95 = s[(n - 1) * 95 / 100];
        let max = s[n - 1];
        let kind = if self.encode_two { "2-renders" } else { "1000-rows" };
        eprintln!("ENCODECRAFT_BENCH {kind} n={} frames={} avg={avg:.2}ms p95={p95:.2}ms max={max:.2}ms", self.rows, n);
        if p95 > 50.0 {
            eprintln!("ENCODECRAFT_BENCH warn: p95 {p95:.1} ms is over the 50 ms CI budget");
        }
        std::process::exit(0);
    }
}
