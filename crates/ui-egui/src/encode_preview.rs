//! Encoding Preview panel — live letterboxed frames of the file being rendered.

use egui::{Color32, CornerRadius, FontId, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, Ui, Vec2};
use encodecraft_ffmpeg::{format_duration, frame_at_25, letterbox_fit, timecode_25};
use encodecraft_queue::{unix_ms, Job};

use crate::theme::Tokens;
use crate::EncodeApp;

pub(crate) struct LastPreview {
    pub id: String,
    pub name: String,
    pub output_w: u32,
    pub output_h: u32,
}

impl EncodeApp {
    pub(crate) fn encode_preview_panel(&mut self, ui: &mut Ui) {
        let t = self.t();
        let mut enabled = self.handle.lock(|s| s.settings.show_encode_preview);
        ui.horizontal(|ui| {
            if ui.checkbox(&mut enabled, "Show preview while rendering").changed() {
                self.handle.lock(|s| s.settings.show_encode_preview = enabled);
                self.handle.mark_dirty();
            }
            ui.label(RichText::new("Off skips the extra FFmpeg grab.").small().color(t.text_faint));
        });
        ui.add_space(4.0);

        let active: Vec<Job> = self.jobs_cache.iter().filter(|j| j.status.is_active()).cloned().collect();
        if active.len() <= 1 {
            self.preview_focus = None;
        }
        if active.is_empty() {
            self.show_idle_preview(ui, &t);
            return;
        }
        if active.len() >= 2 {
            if let Some(focus) = self.preview_focus.clone() {
                if let Some(job) = active.iter().find(|j| j.id == focus).cloned() {
                    ui.horizontal(|ui| {
                        if ui.small_button("Show all").clicked() {
                            self.preview_focus = None;
                        }
                        ui.label(RichText::new(format!("{} rendering", active.len())).small().color(t.text_dim));
                    });
                    self.show_preview_job(ui, &t, &job, true);
                    return;
                }
                self.preview_focus = None;
            }
            self.show_preview_tiles(ui, &t, &active);
            return;
        }
        self.show_preview_job(ui, &t, &active[0], true);
    }

    fn show_idle_preview(&mut self, ui: &mut Ui, t: &Tokens) {
        let last = self.last_preview.as_ref().map(|l| l.id.clone());
        if let Some(id) = last {
            if self.preview_tex.contains_key(&id) {
                ui.label(RichText::new("Last finished frame").small().color(t.text_dim));
                let meta = self.last_preview.as_ref();
                let name = meta.map(|m| m.name.clone()).unwrap_or_default();
                let ow = meta.map(|m| m.output_w).unwrap_or(0);
                let oh = meta.map(|m| m.output_h).unwrap_or(0);
                let pane = preview_pane_size(ui, true);
                self.paint_letterboxed(ui, t, &id, ow, oh, pane, false);
                ui.add_space(4.0);
                ui.label(RichText::new(name).strong().color(t.text));
                return;
            }
        }
        let pane = preview_pane_size(ui, true);
        let (rect, _) = ui.allocate_exact_size(pane, Sense::hover());
        ui.painter().rect_filled(rect, CornerRadius::same(4), t.app_bg);
        ui.painter().rect_stroke(rect, CornerRadius::same(4), Stroke::new(1.0_f32, t.field_border), StrokeKind::Inside);
        ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, "Nothing rendering", FontId::proportional(15.0), t.text_faint);
    }

    fn show_preview_tiles(&mut self, ui: &mut Ui, t: &Tokens, jobs: &[Job]) {
        ui.label(RichText::new(format!("{} rendering — click a tile to enlarge", jobs.len())).small().color(t.text_dim));
        let avail = ui.available_size();
        let cols = if avail.x >= 360.0 { 2.0 } else { 1.0 };
        let gap = 8.0;
        let tile_w = ((avail.x - gap * (cols - 1.0)) / cols).clamp(148.0, 420.0);
        let img_h = (tile_w * 0.48).clamp(72.0, 160.0);
        let tile = Vec2::new(tile_w, img_h);
        let cell_h = img_h + 78.0;
        egui::ScrollArea::vertical().id_salt("enc-preview-tiles").show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = Vec2::new(gap, 8.0);
                for job in jobs {
                    ui.allocate_ui(Vec2::new(tile_w, cell_h), |ui| {
                        ui.vertical(|ui| {
                            let clicked = self.paint_letterboxed(ui, t, &job.id, output_wh(job).0, output_wh(job).1, tile, true);
                            if clicked {
                                self.preview_focus = Some(job.id.clone());
                            }
                            self.preview_stats(ui, t, job, false);
                        });
                    });
                }
            });
        });
    }

    fn show_preview_job(&mut self, ui: &mut Ui, t: &Tokens, job: &Job, large: bool) {
        let pane = preview_pane_size(ui, large);
        let (ow, oh) = output_wh(job);
        if job.preset.format.is_audio_only() {
            let (rect, _) = ui.allocate_exact_size(pane, Sense::hover());
            ui.painter().rect_filled(rect, CornerRadius::same(4), t.app_bg);
            ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, "Audio only — no picture", FontId::proportional(14.0), t.text_faint);
        } else {
            self.paint_letterboxed(ui, t, &job.id, ow, oh, pane, false);
        }
        ui.add_space(6.0);
        self.preview_stats(ui, t, job, true);
    }

    fn paint_letterboxed(&mut self, ui: &mut Ui, t: &Tokens, id: &str, output_w: u32, output_h: u32, pane: Vec2, clickable: bool) -> bool {
        let sense = if clickable { Sense::click() } else { Sense::hover() };
        let (rect, resp) = ui.allocate_exact_size(pane, sense);
        ui.painter().rect_filled(rect, CornerRadius::same(4), t.app_bg);
        ui.painter().rect_stroke(rect, CornerRadius::same(4), Stroke::new(1.0_f32, t.field_border), StrokeKind::Inside);

        let (src_w, src_h) = if output_w > 0 && output_h > 0 {
            (output_w, output_h)
        } else if let Some(tex) = self.preview_tex.get(id) {
            let s = tex.size();
            (s[0] as u32, s[1] as u32)
        } else {
            (16, 9)
        };
        let (fw, fh) = letterbox_fit(rect.width() - 4.0, rect.height() - 4.0, src_w.max(1), src_h.max(1));
        let img = Rect::from_center_size(rect.center(), Vec2::new(fw, fh));

        if let Some(tex) = self.preview_tex.get(id) {
            ui.painter().image(tex.id(), img, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
        } else {
            let msg = if self.jobs_cache.iter().find(|j| j.id == id).is_some_and(|j| j.progress.stage == "Rendering comp") {
                "Rendering composition…"
            } else {
                "Waiting for frame…"
            };
            ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, msg, FontId::proportional(13.0), t.text_faint);
        }

        if src_w > 0 && src_h > 0 {
            let badge = format!("{src_w}×{src_h}");
            let galley = ui.painter().layout_no_wrap(badge, FontId::proportional(11.0), Color32::WHITE);
            let origin = img.min + Vec2::new(6.0, 6.0);
            let br = Rect::from_min_size(origin, galley.size() + Vec2::new(10.0, 5.0));
            ui.painter().rect_filled(br, CornerRadius::same(3), Color32::from_black_alpha(170));
            ui.painter().galley(origin + Vec2::new(5.0, 2.0), galley, Color32::WHITE);
        }
        if clickable && resp.hovered() {
            ui.painter().rect_stroke(rect, CornerRadius::same(4), Stroke::new(1.5_f32, t.link), StrokeKind::Inside);
        }
        resp.clicked()
    }

    fn preview_stats(&self, ui: &mut Ui, t: &Tokens, job: &Job, detailed: bool) {
        let name = job.output.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| job.source.display_name());
        ui.add(egui::Label::new(RichText::new(name).strong().color(t.text)).truncate());
        if !job.progress.stage.is_empty() {
            ui.label(RichText::new(&job.progress.stage).color(t.accent).size(12.0));
        }

        let pct = job.progress.percent.clamp(0.0, 100.0);
        let bar_w = ui.available_width().max(40.0);
        let (bar, _) = ui.allocate_exact_size(Vec2::new(bar_w, 6.0), Sense::hover());
        ui.painter().rect_filled(bar, CornerRadius::same(3), t.raised);
        let fill_w = (bar.width() * (pct / 100.0)).clamp(0.0, bar.width());
        if fill_w > 0.5 {
            let fill = Rect::from_min_size(bar.min, Vec2::new(fill_w, bar.height()));
            ui.painter().rect_filled(fill, CornerRadius::same(3), t.accent);
        }

        let frame = frame_at_25(job.progress.out_time_sec, job.progress.frame);
        let tc = timecode_25(frame);
        let elapsed = job.started_ms.map(|s| (unix_ms().saturating_sub(s) as f64) / 1000.0).unwrap_or(0.0);
        let remain = job.progress.eta_sec.filter(|s| s.is_finite() && *s >= 0.0).map(format_duration).unwrap_or_else(|| "—".into());
        let speed = if job.progress.speed > 0.05 { format!("{:.2}×", job.progress.speed) } else { "—".into() };
        let fps = if job.progress.fps > 0.05 { format!("{:.1} fps", job.progress.fps) } else { "—".into() };
        let br = format_bitrate(job.progress.bitrate_kbps);
        let size = if detailed { 13.0 } else { 11.0 };
        ui.label(RichText::new(format!("{pct:.1}%  ·  Frame {frame}  ·  {tc}")).color(t.text).size(size));
        ui.label(RichText::new(format!("Elapsed {}  ·  Remaining {remain}", format_duration(elapsed))).color(t.text_dim).size(11.0));
        ui.label(RichText::new(format!("{speed}  ·  {fps}  ·  {br}")).color(t.text).size(11.0));
    }
}

fn preview_pane_size(ui: &Ui, large: bool) -> Vec2 {
    let avail = ui.available_size();
    if large {
        let w = avail.x.max(80.0);
        let h = (avail.y - 110.0).clamp(90.0, 720.0);
        Vec2::new(w, h)
    } else {
        Vec2::new(avail.x.max(80.0), 140.0)
    }
}

fn output_wh(job: &Job) -> (u32, u32) {
    (job.preset.video.width.unwrap_or(0), job.preset.video.height.unwrap_or(0))
}

fn format_bitrate(kbps: Option<f32>) -> String {
    match kbps {
        Some(b) if b >= 1000.0 => format!("{:.2} Mbps", b / 1000.0),
        Some(b) if b > 0.0 => format!("{:.0} kbps", b),
        _ => "— kbps".into(),
    }
}
