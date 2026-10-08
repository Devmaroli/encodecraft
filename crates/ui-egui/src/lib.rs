//! EncodeCraft desktop UI — Media Encoder-style queue, preset browser, encoding panel.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod dock;
mod encode_preview;
mod panels;
mod perf;
mod theme;
mod ux;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use egui::{Color32, CornerRadius, Pos2, RichText, Stroke, StrokeKind, Ui, Vec2};
use encodecraft_engine::{classify_dropped_paths, seed_preset_ui_demo, seed_stress_queue, DropOutcome, Handle, IoCmd, UiEvent};
use encodecraft_job::{ScaleMode, PRODUCT};
use encodecraft_presets::{ExportPreset, FrameRate, OutputFormat, PresetReason, Quality};
use encodecraft_queue::{Job, JobStatus};

use crate::dock::{DockCmd, LayoutSession, PanelKind};
use crate::theme::{ThemeKind, Tokens};

pub struct EncodeApp {
    pub(crate) handle: Handle,
    pub(crate) tokens: Tokens,
    pub(crate) layout: LayoutSession,
    pub(crate) pending: Vec<DockCmd>,
    pub(crate) preset_filter: String,
    log_filter: String,
    pub(crate) show_save_as: bool,
    pub(crate) save_as_name: String,
    pub(crate) show_manage: bool,
    pub(crate) rename_idx: Option<usize>,
    pub(crate) rename_buf: String,
    pub(crate) show_preset_edit: bool,
    show_about: bool,
    pub(crate) edit_preset: Option<ExportPreset>,
    thumb_tex: HashMap<String, egui::TextureHandle>,
    preview_tex: HashMap<String, egui::TextureHandle>,
    preview_focus: Option<String>,
    last_preview: Option<encode_preview::LastPreview>,
    pub(crate) status: String,
    drop_notice: Option<(String, Instant)>,
    ecproj_picker: Option<EcprojPicker>,
    pub(crate) show_rules: bool,
    pub(crate) selected_ids: Vec<String>,
    pub(crate) collapsed: HashSet<String>,
    pub(crate) browser_selected: Option<String>,
    pub(crate) toast: Option<ux::UndoToast>,
    pub(crate) preset_popup_for: Option<String>,
    pub(crate) format_popup_for: Option<String>,
    pub(crate) popup_anchor: Option<Pos2>,
    pub(crate) preset_search: String,
    pub(crate) shift_anchor: Option<String>,
    pub(crate) try_screen: String,
    pub(crate) try_result: Option<String>,
    pub(crate) rules_new_ultra: String,
    pub(crate) show_screen_picker: bool,
    pub(crate) jobs_cache: Vec<Job>,
    snap_rev: u64,
    queue_running: bool,
    ffmpeg_ok: bool,
    ffmpeg_msg: String,
    thumb_inflight: HashSet<String>,
    theme_applied: bool,
    perf: perf::PerfHud,
    bench: Option<perf::BenchRun>,
}

struct EcprojPicker {
    path: PathBuf,
    comps: Vec<String>,
    selected: Vec<bool>,
    custom: String,
    parse_note: Option<String>,
}

impl EncodeApp {
    pub fn new(handle: Handle) -> Self {
        if std::env::var("ENCODECRAFT_DEMO").ok().as_deref() == Some("1") {
            let _ = handle.lock(|s| seed_preset_ui_demo(s));
        }
        if let Ok(raw) = std::env::var("ENCODECRAFT_BENCH") {
            let n = std::env::var("ENCODECRAFT_BENCH_ROWS").ok().and_then(|s| s.parse().ok()).unwrap_or(1000);
            let _ = handle.lock(|s| seed_stress_queue(s, n));
            if raw == "encode2" {
                handle.lock(|s| {
                    // Keep the worker idle so the bench measures UI + progress
                    // chrome, not FFmpeg startup on missing stress sources.
                    s.queue.running = false;
                    for (i, j) in s.queue.jobs.iter_mut().take(2).enumerate() {
                        j.status = JobStatus::Encoding;
                        j.progress.percent = 10.0 + (i as f32) * 20.0;
                        j.progress.speed = 1.2;
                    }
                });
            }
        }
        let selected = handle.lock(|s| s.selected.clone());
        Self {
            handle,
            tokens: Tokens::for_kind(ThemeKind::Dark),
            layout: LayoutSession::load(),
            pending: Vec::new(),
            preset_filter: String::new(),
            log_filter: String::new(),
            show_save_as: false,
            save_as_name: String::new(),
            show_manage: false,
            rename_idx: None,
            rename_buf: String::new(),
            show_preset_edit: false,
            show_about: false,
            edit_preset: None,
            thumb_tex: HashMap::new(),
            preview_tex: HashMap::new(),
            preview_focus: None,
            last_preview: None,
            status: String::new(),
            drop_notice: None,
            ecproj_picker: None,
            show_rules: false,
            selected_ids: selected.clone().into_iter().collect(),
            collapsed: HashSet::new(),
            browser_selected: None,
            toast: None,
            preset_popup_for: None,
            format_popup_for: None,
            popup_anchor: None,
            preset_search: String::new(),
            shift_anchor: selected,
            try_screen: String::new(),
            try_result: None,
            rules_new_ultra: String::new(),
            show_screen_picker: false,
            jobs_cache: Vec::new(),
            snap_rev: 0,
            queue_running: false,
            ffmpeg_ok: true,
            ffmpeg_msg: String::new(),
            thumb_inflight: HashSet::new(),
            theme_applied: false,
            perf: perf::PerfHud::default(),
            bench: perf::BenchRun::from_env(),
        }
    }

    pub(crate) fn t(&self) -> Tokens {
        self.tokens
    }

    fn poll_background(&mut self, ctx: &egui::Context) {
        for ev in self.handle.poll_ui_events() {
            match ev {
                UiEvent::Changed => {
                    self.snap_rev = 0;
                }
                UiEvent::Thumb { id, rgba, w, h } => {
                    self.thumb_inflight.remove(&id);
                    let color = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &rgba);
                    let tex = ctx.load_texture(format!("th-{id}"), color, egui::TextureOptions::LINEAR);
                    self.thumb_tex.insert(id, tex);
                }
                UiEvent::PreviewFrame { id, name, rgba, w, h, output_w, output_h } => {
                    let color = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &rgba);
                    let tex = ctx.load_texture(format!("prev-{id}"), color, egui::TextureOptions::LINEAR);
                    self.preview_tex.insert(id.clone(), tex);
                    self.last_preview = Some(encode_preview::LastPreview { id, name, output_w, output_h });
                }
                UiEvent::Ecproj { path, comps, note } => {
                    let selected = if comps.is_empty() { Vec::new() } else { vec![true; comps.len()] };
                    self.ecproj_picker = Some(EcprojPicker { path, comps, selected, custom: String::new(), parse_note: note });
                }
                UiEvent::Dropped(outcome) => {
                    self.apply_drop_outcome(outcome, 0);
                }
                UiEvent::Slow { name, ms } => self.perf.push_slow(&name, ms),
            }
        }
    }

    fn refresh_snapshot(&mut self) {
        let rev = self.handle.revision();
        if rev == self.snap_rev && !self.jobs_cache.is_empty() {
            return;
        }
        let (jobs, running, ff) = self.handle.lock(|s| {
            (
                s.queue.jobs.iter().map(Job::for_ui).collect::<Vec<_>>(),
                s.queue.running,
                match &s.ffmpeg {
                    Ok(f) => (true, f.ok_message()),
                    Err(e) => (false, e.to_string()),
                },
            )
        });
        self.jobs_cache = jobs;
        self.queue_running = running;
        self.ffmpeg_ok = ff.0;
        self.ffmpeg_msg = ff.1;
        self.snap_rev = rev;
    }

    fn schedule_repaint(&self, ctx: &egui::Context) {
        if egui::DragAndDrop::payload::<ux::QueueDrag>(ctx).is_some() {
            ctx.request_repaint();
            return;
        }
        let encoding = self.jobs_cache.iter().any(|j| j.status.is_active()) || self.queue_running;
        let pending_meta = self.jobs_cache.iter().any(|j| j.metadata_pending);
        let toast = self.toast.as_ref().map(|t| t.until.saturating_duration_since(Instant::now()));
        if encoding {
            ctx.request_repaint_after(Duration::from_millis(120));
        } else if pending_meta {
            ctx.request_repaint_after(Duration::from_millis(200));
        } else if let Some(left) = toast {
            if !left.is_zero() {
                ctx.request_repaint_after(left.min(Duration::from_millis(250)));
            }
        }
        if self.bench.is_some() {
            ctx.request_repaint();
        }
        if self.perf.enabled {
            ctx.request_repaint_after(Duration::from_millis(250));
        }
    }
}

impl eframe::App for EncodeApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if ctx.input(|i| i.viewport().close_requested()) && ctx.viewport_id() == egui::ViewportId::ROOT {
            self.layout.save_now();
            self.handle.shutdown();
            std::process::exit(0);
        }
        self.perf.begin_frame();
        if !self.theme_applied {
            theme::apply(ctx, &self.tokens);
            self.theme_applied = true;
        }
        self.poll_background(ctx);
        self.refresh_snapshot();
        self.schedule_repaint(ctx);
        self.handle_dock_keys(ctx);

        if ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::Z)) {
            self.undo_toast();
        }
        if ctx.input(|i| i.modifiers.alt && i.key_pressed(egui::Key::ArrowUp)) {
            self.move_selected_rows(-1);
        }
        if ctx.input(|i| i.modifiers.alt && i.key_pressed(egui::Key::ArrowDown)) {
            self.move_selected_rows(1);
        }
        self.ingest_drops(ctx);
        self.drop_overlay(ctx);

        self.menu(ctx);
        self.toolbar(ctx);
        self.workspace_bar(ctx);
        let t = self.t();
        egui::CentralPanel::default().frame(egui::Frame::NONE.fill(t.app_bg)).show(ctx, |ui| {
            self.show_workspace(ui);
        });
        self.show_floats(ctx);
        self.apply_dock_cmds(ctx);
        self.layout.save_if_due();
        self.drop_notice_banner(ctx);
        self.ffmpeg_banner(ctx);
        self.dialogs(ctx);
        self.perf.end_frame();
        self.perf.show(ctx);
        if let Some(bench) = self.bench.as_mut() {
            bench.record(Duration::from_secs_f32((self.perf.last_ms / 1000.0).max(0.0)));
            if bench.remaining == 0 {
                let done = self.bench.take();
                if let Some(b) = done {
                    b.report_and_exit();
                }
            }
        }
        let _ = t;
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.layout.save_now();
        self.handle.shutdown();
        std::process::exit(0);
    }
}

impl EncodeApp {
    fn ingest_drops(&mut self, ctx: &egui::Context) {
        let (paths, nameless) = ctx.input(|i| {
            let mut paths = Vec::new();
            let mut nameless = 0usize;
            for f in &i.raw.dropped_files {
                match &f.path {
                    Some(p) => paths.push(p.clone()),
                    None => nameless += 1,
                }
            }
            (paths, nameless)
        });
        if paths.is_empty() && nameless == 0 {
            return;
        }
        if paths.is_empty() {
            self.set_drop_notice("That drop had no file path. Save the EffectCraft project and drop the .ecproj from Explorer, or drop media files.");
            return;
        }
        let mut files = Vec::new();
        let mut dirs = Vec::new();
        for p in paths {
            if p.is_dir() {
                dirs.push(p);
            } else {
                files.push(p);
            }
        }
        if !dirs.is_empty() {
            self.handle.submit_io(IoCmd::Classify(dirs));
        }
        if !files.is_empty() {
            self.apply_drop_outcome(classify_dropped_paths(&files), nameless);
        } else if nameless > 0 {
            self.set_drop_notice("Some dropped items had no file path and were skipped.");
        }
    }

    fn apply_drop_outcome(&mut self, outcome: DropOutcome, nameless: usize) {
        let preset = self.handle.lock(|s| s.settings.default_preset_id.clone());
        let mut bits = Vec::new();
        if !outcome.media.is_empty() || !outcome.sequences.is_empty() {
            match self.handle.lock(|s| s.add_drop_outcome(&outcome, &preset)) {
                Ok(ids) => {
                    self.handle.note_change();
                    self.handle.schedule_probe(ids.clone());
                    bits.push(format!("Queued {} media item(s)", ids.len()));
                }
                Err(e) => self.set_drop_notice(format!("Could not queue dropped media: {e}")),
            }
        }
        if let Some(msg) = outcome.unsupported_message() {
            self.set_drop_notice(msg);
        } else if nameless > 0 {
            self.set_drop_notice("Some dropped items had no file path and were skipped.");
        }
        if !outcome.projects.is_empty() {
            self.open_ecproj_picker(outcome.projects);
        }
        if !bits.is_empty() {
            self.status = bits.join(" · ");
        }
    }

    fn open_ecproj_picker(&mut self, projects: Vec<PathBuf>) {
        let Some(path) = projects.into_iter().next() else {
            return;
        };
        self.ecproj_picker = Some(EcprojPicker {
            path: path.clone(),
            comps: Vec::new(),
            selected: Vec::new(),
            custom: String::new(),
            parse_note: Some("Reading project…".into()),
        });
        self.handle.submit_io(IoCmd::ListEcproj(path));
    }

    fn set_drop_notice(&mut self, msg: impl Into<String>) {
        let msg = msg.into();
        self.status = msg.clone();
        self.drop_notice = Some((msg, Instant::now()));
    }

    fn drop_overlay(&self, ctx: &egui::Context) {
        let hovered = ctx.input(|i| i.raw.hovered_files.clone());
        if hovered.is_empty() {
            return;
        }
        let has_ecproj = hovered.iter().any(|f| f.path.as_ref().is_some_and(|p| p.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("ecproj"))));
        let screen = ctx.screen_rect();
        let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("drop-overlay")));
        let fill = Color32::from_rgba_unmultiplied(0x24, 0x48, 0xF5, 42);
        painter.rect_filled(screen, CornerRadius::ZERO, fill);
        painter.rect_stroke(
            screen.shrink(10.0),
            CornerRadius::same(10),
            Stroke::new(3.0_f32, self.t().accent),
            StrokeKind::Inside,
        );
        let label = if has_ecproj {
            "Drop EffectCraft project to pick compositions"
        } else {
            "Drop media to add to the queue"
        };
        let galley = painter.layout_no_wrap(label.to_string(), egui::FontId::proportional(22.0), Color32::WHITE);
        let pos = screen.center() - galley.size() * 0.5;
        painter.galley(pos, galley, Color32::WHITE);
        ctx.request_repaint();
    }

    fn drop_notice_banner(&mut self, ctx: &egui::Context) {
        let Some((msg, at)) = self.drop_notice.clone() else {
            return;
        };
        if at.elapsed() > Duration::from_secs(10) {
            self.drop_notice = None;
            return;
        }
        egui::TopBottomPanel::top("drop-notice").frame(egui::Frame::NONE.fill(Color32::from_rgb(72, 40, 20)).inner_margin(8.0)).show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(&msg).color(Color32::from_rgb(255, 220, 180)).size(13.0));
                if ui.small_button("Dismiss").clicked() {
                    self.drop_notice = None;
                }
            });
        });
    }

    fn menu(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("menu").exact_height(28.0).show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.menu_button("File", |ui| {
                    if ui.button("Add source…").clicked() {
                        self.pick_sources();
                        ui.close();
                    }
                    ui.separator();
                    if ui.button("Import preset…").clicked() {
                        self.import_preset();
                        ui.close();
                    }
                    if ui.button("Export selected preset…").clicked() {
                        self.export_preset();
                        ui.close();
                    }
                    ui.separator();
                    if ui.button("Quit").clicked() {
                        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });
                ui.menu_button("Queue", |ui| {
                    if ui.button("Start queue").clicked() {
                        self.handle.lock(|s| s.queue.start());
                        ui.close();
                    }
                    if ui.button("Pause queue").clicked() {
                        self.handle.lock(|s| s.queue.running = false);
                        ui.close();
                    }
                    if ui.button("Stop all").clicked() {
                        self.handle.stop_all();
                        ui.close();
                    }
                    ui.separator();
                    if ui.button("Remove finished").clicked() {
                        self.handle.lock(|s| s.queue.remove_finished());
                        ui.close();
                    }
                    ui.separator();
                    if ui.button("Auto-assign rules…").clicked() {
                        self.show_rules = true;
                        ui.close();
                    }
                });
                ui.menu_button("Watch", |ui| {
                    if ui.button("Watch folders…").clicked() {
                        if !self.layout.panel_visible(PanelKind::WatchFolders) {
                            self.pending.push(DockCmd::Toggle(PanelKind::WatchFolders));
                        }
                        ui.close();
                    }
                });
                self.window_menu(ui);
                ui.menu_button("Help", |ui| {
                    if ui.button("About EncodeCraft").clicked() {
                        self.show_about = true;
                        ui.close();
                    }
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(RichText::new(PRODUCT).color(self.t().text_dim).size(12.0));
                });
            });
        });
    }

    fn toolbar(&mut self, ctx: &egui::Context) {
        let t = self.t();
        egui::TopBottomPanel::top("toolbar").exact_height(40.0).frame(egui::Frame::NONE.fill(t.header_bg).inner_margin(egui::Margin::symmetric(8, 6))).show(
            ctx,
            |ui| {
                ui.horizontal(|ui| {
                    let running = self.queue_running;
                    let start = egui::Button::new(if running { "  Queue running  " } else { "  Start queue  " });
                    if ui.add(start.fill(if running { t.cache_green } else { t.accent })).clicked() {
                        self.handle.lock(|s| s.queue.start());
                        self.handle.note_change();
                    }
                    if ui.button("  Pause  ").clicked() {
                        self.handle.lock(|s| s.queue.running = false);
                        self.handle.note_change();
                    }
                    if ui.button("  Stop  ").clicked() {
                        self.handle.stop_all();
                    }
                    ui.separator();
                    if ui.button("  + Add  ").clicked() {
                        self.pick_sources();
                    }
                    if ui.button("Duplicate").clicked() {
                        if let Some(id) = self.handle.lock(|s| s.selected.clone()) {
                            if let Ok(n) = self.handle.lock(|s| s.queue.duplicate(&id)) {
                                self.handle.lock(|s| s.selected = Some(n));
                                self.handle.note_change();
                            }
                        }
                    }
                    if ui.button("Remove").clicked() {
                        if let Some(id) = self.handle.lock(|s| s.selected.clone()) {
                            let _ = self.handle.lock(|s| s.queue.remove(&id));
                            self.handle.note_change();
                        }
                    }
                    if ui.button("  Up  ").on_hover_text("Move selected rows up (Alt+↑)").clicked() {
                        self.move_selected_rows(-1);
                    }
                    if ui.button("  Down  ").on_hover_text("Move selected rows down (Alt+↓)").clicked() {
                        self.move_selected_rows(1);
                    }
                    ui.separator();
                    ui.label("Render");
                    let mut n = self.handle.lock(|s| s.settings.max_parallel as i32);
                    if ui.small_button("−").clicked() {
                        n = (n - 1).max(1);
                    }
                    let dv = ui.add(egui::DragValue::new(&mut n).range(1..=8).prefix("").suffix(""));
                    let _ = dv.clone().on_hover_text("How many files encode at the same time");
                    if ui.small_button("+").clicked() {
                        n = (n + 1).min(8);
                    }
                    ui.label("at once");
                    if n as usize != self.handle.lock(|s| s.settings.max_parallel) {
                        self.handle.lock(|s| {
                            s.settings.max_parallel = n as usize;
                            s.queue.max_parallel = n as usize;
                        });
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(RichText::new(&self.status).color(t.text_dim).size(12.0));
                    });
                });
            },
        );
    }

    fn workspace_bar(&mut self, ctx: &egui::Context) {
        let t = self.t();
        egui::TopBottomPanel::top("workspace-bar").exact_height(30.0).frame(egui::Frame::NONE.fill(t.header_bg).inner_margin(egui::Margin::symmetric(8, 4))).show(ctx, |ui| {
            ui.horizontal(|ui| {
                self.workspace_switcher(ui);
            });
        });
    }

    pub(crate) fn encode_panel(&mut self, ui: &mut Ui) {
        let t = self.t();
        let Some(mut job) = selected_job(&self.handle) else {
            ui.label(RichText::new("Select a queue item to edit export settings.").color(t.text_dim));
            return;
        };
        let original = (
            job.preset.clone(),
            job.scale,
            job.trim,
            job.preset_overridden,
            job.preset_reason,
            job.encode_warning.clone(),
        );
        ui.horizontal(|ui| {
            ui.label("Preset");
            let presets = self.handle.lock(|s| s.presets.all());
            let current = job.preset.name.clone();
            egui::ComboBox::from_id_salt("job-preset").selected_text(&current).show_ui(ui, |ui| {
                for p in &presets {
                    if ui.selectable_label(p.id == job.preset.id, &p.name).clicked() {
                        let w = job.preset.video.width;
                        let h = job.preset.video.height;
                        let mut next = p.clone();
                        next.video.width = w;
                        next.video.height = h;
                        next.video.scale = job.scale;
                        self.handle.lock(|s| s.presets.remember_manual_choice(&job.source, job.screen.as_ref(), p));
                        job.preset = next;
                        job.preset_overridden = true;
                        job.preset_reason = Some(PresetReason::Manual);
                    }
                }
            });
            if let Some(r) = job.preset_reason {
                let label = if job.preset_overridden { "override".into() } else { r.label().to_string() };
                ui.label(RichText::new(format!("({label})")).color(t.text_dim).italics());
            }
            if job.preset_overridden && ui.small_button("Reset auto").clicked() {
                self.handle.lock(|s| s.presets.clear_sticky(&job.source, job.screen.as_ref()));
                if let Some(pick) = self.handle.lock(|s| {
                    let px = match (job.preset.video.width, job.preset.video.height) {
                        (Some(w), Some(h)) if w > 0 && h > 0 => Some((w, h)),
                        _ => None,
                    };
                    s.presets.screen_rules.pick(job.screen.as_ref(), &job.source.display_name(), px)
                }) {
                    if let Ok(pr) = self.handle.lock(|s| s.presets.get(&pick.preset_id)) {
                        let w = job.preset.video.width;
                        let h = job.preset.video.height;
                        job.preset = pr;
                        job.preset.video.width = w;
                        job.preset.video.height = h;
                        job.preset.video.scale = job.scale;
                        job.preset_overridden = false;
                        job.preset_reason = Some(pick.reason);
                    }
                }
            }
            ui.separator();
            ui.label("Output");
            ui.add(egui::Label::new(RichText::new(job.output.to_string_lossy().to_string()).color(t.link)).truncate());
        });
        if let Some(w) = &job.encode_warning {
            ui.label(RichText::new(w).color(t.warning).size(12.0));
        }
        ui.add_space(4.0);
        egui::Grid::new("enc-grid").num_columns(4).spacing([12.0, 6.0]).show(ui, |ui| {
            ui.label("Format");
            let mut fmt = job.preset.format;
            egui::ComboBox::from_id_salt("fmt").selected_text(fmt.label()).show_ui(ui, |ui| {
                for f in OutputFormat::all() {
                    ui.selectable_value(&mut fmt, *f, f.label());
                }
            });
            if fmt != job.preset.format {
                job.preset.format = fmt;
            }
            ui.label("Scale");
            let mut scale = job.scale;
            egui::ComboBox::from_id_salt("scale").selected_text(scale.as_str()).show_ui(ui, |ui| {
                for m in [ScaleMode::Fit, ScaleMode::Fill, ScaleMode::Stretch, ScaleMode::Scale] {
                    ui.selectable_value(&mut scale, m, m.as_str());
                }
            });
            job.scale = scale;
            job.preset.video.scale = scale;
            ui.end_row();

            ui.label("Width");
            let mut w = job.preset.video.width.unwrap_or(0) as i32;
            if ui.add(egui::DragValue::new(&mut w).range(0..=16384).suffix(" px")).changed() {
                job.preset.video.width = if w == 0 { None } else { Some(w as u32) };
            }
            ui.label("Height");
            let mut h = job.preset.video.height.unwrap_or(0) as i32;
            if ui.add(egui::DragValue::new(&mut h).range(0..=16384).suffix(" px")).changed() {
                job.preset.video.height = if h == 0 { None } else { Some(h as u32) };
            }
            ui.end_row();

            ui.label("Frame rate");
            let mut fps_src = matches!(job.preset.video.frame_rate, FrameRate::Source);
            if ui.checkbox(&mut fps_src, "Match source").changed() {
                job.preset.video.frame_rate = if fps_src { FrameRate::Source } else { FrameRate::Custom(30.0) };
            }
            if let FrameRate::Custom(ref mut f) = job.preset.video.frame_rate {
                ui.add(egui::DragValue::new(f).range(1.0..=120.0).suffix(" fps"));
            } else {
                ui.label("—");
            }
            ui.end_row();

            ui.label("Quality");
            match &mut job.preset.video.quality {
                Quality::Crf { value } => {
                    ui.horizontal(|ui| {
                        ui.label("CRF");
                        ui.add(egui::Slider::new(value, 0..=51));
                    });
                }
                Quality::Bitrate { kbps } => {
                    ui.horizontal(|ui| {
                        ui.label("kbps");
                        ui.add(egui::DragValue::new(kbps).range(100..=100_000));
                    });
                }
                Quality::Quality { value } => {
                    ui.add(egui::Slider::new(value, 1..=100).text("quality"));
                }
                Quality::Vbr2Pass { target_mbps, max_mbps } => {
                    ui.horizontal(|ui| {
                        ui.label("Target Mbps");
                        ui.add(egui::DragValue::new(target_mbps).range(1.0..=200.0).speed(0.5));
                        ui.label("Max Mbps");
                        ui.add(egui::DragValue::new(max_mbps).range(1.0..=250.0).speed(0.5));
                    });
                }
            }
            if ui.button("CRF / bitrate / 2-pass").clicked() {
                job.preset.video.quality = match job.preset.video.quality {
                    Quality::Crf { .. } => Quality::bitrate(5000),
                    Quality::Bitrate { .. } => Quality::vbr_2pass(5.0, 6.0),
                    _ => Quality::crf(23),
                };
            }
            ui.end_row();

            ui.label("Audio");
            ui.checkbox(&mut job.preset.audio.enabled, "Include audio");
            ui.label("Audio kbps");
            ui.add(egui::DragValue::new(&mut job.preset.audio.bitrate_kbps).range(32..=512));
            ui.end_row();

            ui.label("Trim in");
            ui.add(egui::DragValue::new(&mut job.trim.in_sec).range(0.0..=86_400.0).suffix(" s").speed(0.1));
            ui.label("Trim out");
            let mut out_on = job.trim.out_sec.is_some();
            if ui.checkbox(&mut out_on, "Set out").changed() {
                job.trim.out_sec = if out_on { Some(job.trim.in_sec + 5.0) } else { None };
            }
            if let Some(ref mut o) = job.trim.out_sec {
                ui.add(egui::DragValue::new(o).range(0.0..=86_400.0).suffix(" s").speed(0.1));
            }
            ui.end_row();
        });
        let id = job.id.clone();
        job.encode_warning = encodecraft_ffmpeg::h264_unencodable_message(
            &job.preset,
            job.preset.video.width.unwrap_or(0),
            job.preset.video.height.unwrap_or(0),
        );
        let changed = job.preset != original.0
            || job.scale != original.1
            || job.trim != original.2
            || job.preset_overridden != original.3
            || job.preset_reason != original.4
            || job.encode_warning != original.5;
        if changed {
            self.handle.lock(|s| {
                if let Some(j) = s.queue.jobs.iter_mut().find(|j| j.id == id) {
                    j.preset = job.preset;
                    j.scale = job.scale;
                    j.trim = job.trim;
                    j.preset_overridden = job.preset_overridden;
                    j.preset_reason = job.preset_reason;
                    j.encode_warning = job.encode_warning;
                }
            });
            self.handle.note_change();
        }
    }

    pub(crate) fn preview_panel(&mut self, ui: &mut Ui) {
        let t = self.t();
        let Some(job) = selected_job(&self.handle) else {
            ui.label(RichText::new("Select a job to preview the source.").color(t.text_dim));
            return;
        };
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.label(RichText::new(job.source.display_name()).strong());
                ui.label(RichText::new(job.source.path_hint().display().to_string()).color(t.link));
                if let (Some(w), Some(h)) = (job.preset.video.width, job.preset.video.height) {
                    if let Some(d) = job.duration_sec {
                        ui.label(format!("{w}×{h} · {}", encodecraft_ffmpeg::format_duration(d)));
                    } else if job.metadata_pending {
                        ui.label(format!("{w}×{h} · probing…"));
                    } else {
                        ui.label(format!("{w}×{h}"));
                    }
                } else if job.metadata_pending {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Reading source…");
                    });
                }
                ui.label(RichText::new(format!("-> {}", job.output.display())).color(t.link));
            });
            ui.add_space(16.0);
            if let Some(tex) = self.ensure_thumb(ui.ctx(), &job) {
                ui.image((tex.id(), Vec2::new(240.0, 135.0)));
            } else if self.thumb_inflight.contains(&job.id) {
                ui.allocate_ui(Vec2::new(240.0, 135.0), |ui| {
                    ui.centered_and_justified(|ui| ui.spinner());
                });
            } else {
                ui.allocate_ui(Vec2::new(240.0, 135.0), |ui| {
                    ui.centered_and_justified(|ui| ui.label(RichText::new("No thumbnail").color(t.text_faint)));
                });
            }
        });
    }

    pub(crate) fn log_panel(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.label("Filter");
            ui.add(egui::TextEdit::singleline(&mut self.log_filter).desired_width(200.0));
            if ui.button("Clear job log").clicked() {
                if let Some(id) = self.handle.lock(|s| s.selected.clone()) {
                    self.handle.lock(|s| {
                        if let Some(j) = s.queue.jobs.iter_mut().find(|j| j.id == id) {
                            j.log.clear();
                        }
                    });
                }
            }
        });
        let (job_log, app_log) = self.handle.lock(|s| {
            let job = s.selected.as_ref().and_then(|id| s.queue.jobs.iter().find(|j| j.id == *id)).map(|j| j.log.clone()).unwrap_or_default();
            (job, s.log.join("\n"))
        });
        let text = if job_log.is_empty() { app_log } else { job_log };
        let shown: String = if self.log_filter.is_empty() {
            text
        } else {
            text.lines().filter(|l| l.to_ascii_lowercase().contains(&self.log_filter.to_ascii_lowercase())).collect::<Vec<_>>().join("\n")
        };
        let lines: Vec<&str> = shown.lines().collect();
        let n = lines.len().max(1);
        egui::ScrollArea::vertical().id_salt("log").stick_to_bottom(true).show_rows(ui, 16.0, n, |ui, range| {
            for i in range {
                if let Some(line) = lines.get(i) {
                    ui.monospace(*line);
                }
            }
        });
    }

    pub(crate) fn settings_panel(&mut self, ui: &mut Ui) {
        let mut s = self.handle.lock(|x| x.settings.clone());
        ui.label(RichText::new("Defaults").strong());
        ui.label("Default output folder");
        ui.radio_value(&mut s.output_beside_source, true, "Output folder next to the project");
        ui.label(RichText::new("EffectCraft projects and dropped files write to an Output folder beside the source.").small().color(self.t().text_dim));
        ui.horizontal(|ui| {
            ui.radio_value(&mut s.output_beside_source, false, "Fixed folder…");
            let mut out = s.output_dir.to_string_lossy().into_owned();
            let edit = ui.add_enabled(!s.output_beside_source, egui::TextEdit::singleline(&mut out).desired_width(320.0));
            if edit.changed() {
                s.output_dir = PathBuf::from(out);
            }
            if ui.add_enabled(!s.output_beside_source, egui::Button::new("Browse…")).clicked() {
                if let Some(d) = rfd::FileDialog::new().pick_folder() {
                    s.output_dir = d;
                    s.output_beside_source = false;
                }
            }
        });
        ui.horizontal(|ui| {
            ui.label("Naming");
            ui.add(egui::TextEdit::singleline(&mut s.naming).desired_width(280.0).hint_text("{name}_{width}x{height}"));
            ui.label(RichText::new("{name} {width} {height} {preset} {codec} {date} {screen}").small().color(self.t().text_faint));
        });
        ui.horizontal(|ui| {
            ui.label("Default preset");
            let presets = self.handle.lock(|x| x.presets.all());
            let cur = presets.iter().find(|p| p.id == s.default_preset_id).map(|p| p.name.clone()).unwrap_or_else(|| s.default_preset_id.clone());
            egui::ComboBox::from_id_salt("default-preset").selected_text(&cur).show_ui(ui, |ui| {
                for p in &presets {
                    if ui.selectable_label(p.id == s.default_preset_id, &p.name).clicked() {
                        s.default_preset_id = p.id.clone();
                    }
                }
            });
            ui.label(RichText::new("used when a file has no screen name").small().color(self.t().text_dim));
        });
        ui.horizontal(|ui| {
            ui.label("Control port");
            ui.add(egui::DragValue::new(&mut s.http_port).range(1024..=65535));
            ui.label(RichText::new("EffectCraft posts to 127.0.0.1:this").small());
        });
        ui.checkbox(&mut s.auto_start_queue, "Start the queue when a job arrives from EffectCraft / the CLI");
        ui.add_space(8.0);
        ui.label(RichText::new("Encoding preview").strong());
        let mut preview = s.show_encode_preview;
        if ui.checkbox(&mut preview, "Show preview while rendering").changed() {
            s.show_encode_preview = preview;
            self.handle.lock(|sess| sess.settings.show_encode_preview = preview);
            self.handle.mark_dirty();
        }
        ui.label(
            RichText::new("Decodes a low-res frame from the encode a couple of times a second. Turn off to skip that extra FFmpeg work.")
                .small()
                .color(self.t().text_dim),
        );
        ui.add_space(8.0);
        ui.label(RichText::new("Studio screen assignment").strong());
        ui.label(
            RichText::new("Sized jobs pick Better Res (ultra wide / listed screens) or Installation. Open the rules editor to change the lists — no typing JSON.")
                .small()
                .color(self.t().text_dim),
        );
        if ui.button("Auto-assign rules…").clicked() {
            self.show_rules = true;
        }
        ui.add_space(8.0);
        ui.label(RichText::new("One-off presets").strong());
        ui.label(
            RichText::new("These are never remembered as a manual choice. The next send of that comp goes back to Auto (Better Res / Installation) or the default.")
                .small()
                .color(self.t().text_dim),
        );
        let one_offs = self.handle.lock(|x| x.presets.one_off_presets());
        if one_offs.is_empty() {
            ui.label(RichText::new("None yet — tick One-off on a preset, or name one Sultan.").small().color(self.t().text_dim));
        } else {
            for p in &one_offs {
                ui.label(format!("• {}  ({})", p.name, p.id));
            }
        }
        if ui.button("Apply settings").clicked() {
            self.handle.lock(|sess| {
                sess.settings = s.clone();
            });
            self.handle.mark_dirty();
            self.status = "Settings saved".into();
        }
    }

    fn ffmpeg_banner(&self, ctx: &egui::Context) {
        if self.ffmpeg_ok {
            return;
        }
        let t = self.t();
        egui::TopBottomPanel::bottom("ff-missing").frame(egui::Frame::NONE.fill(Color32::from_rgb(80, 24, 20)).inner_margin(8.0)).show(ctx, |ui| {
            ui.label(RichText::new("FFmpeg not found").strong().color(Color32::WHITE));
            ui.label(RichText::new(&self.ffmpeg_msg).color(t.tab_text_active).size(12.0));
        });
    }

    fn dialogs(&mut self, ctx: &egui::Context) {
        if self.show_about {
            egui::Window::new("About EncodeCraft").collapsible(false).resizable(false).open(&mut self.show_about).show(ctx, |ui| {
                ui.label(RichText::new("EncodeCraft").heading());
                ui.label("A Media Encoder-style companion to EffectCraft.");
                ui.label("Queue · presets · DOOH screen sizes · watch folders · EffectCraft handoff.");
                ui.label("Independent project; not affiliated with Adobe or ArtCraft.");
                ui.hyperlink("https://github.com/storytold/effectcraft");
            });
        }
        self.save_as_dialog(ctx);
        self.manage_dialog(ctx);
        self.preset_edit_window(ctx);
        self.rules_window(ctx);
        self.screen_library_picker(ctx);
        self.ecproj_dialog(ctx);
    }

    fn ecproj_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut picker) = self.ecproj_picker.take() else {
            return;
        };
        let mut open = true;
        let mut queue = false;
        let mut cancel = false;
        let mut queued_ok = false;
        let title = format!("Queue EffectCraft project — {}", picker.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| picker.path.display().to_string()));
        egui::Window::new(title).open(&mut open).collapsible(false).resizable(true).default_width(420.0).show(ctx, |ui| {
            ui.label("Choose which composition(s) to send to the encode queue. EncodeCraft will render each with effectcraft-cli, then encode with the default preset.");
            if let Some(note) = &picker.parse_note {
                ui.label(RichText::new(note).color(self.t().warning).size(12.0));
            }
            if picker.comps.is_empty() {
                ui.horizontal(|ui| {
                    ui.label("Composition name (optional)");
                    ui.add(egui::TextEdit::singleline(&mut picker.custom).desired_width(220.0).hint_text("default / first"));
                });
            } else {
                if ui.button("Select all").clicked() {
                    for f in &mut picker.selected {
                        *f = true;
                    }
                }
                ui.add_space(4.0);
                for (i, name) in picker.comps.iter().enumerate() {
                    if let Some(flag) = picker.selected.get_mut(i) {
                        ui.checkbox(flag, name);
                    }
                }
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Queue selected").clicked() {
                    queue = true;
                }
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
            });
        });
        if queue {
            let preset = self.handle.lock(|s| s.settings.default_preset_id.clone());
            let comps: Vec<String> = if picker.comps.is_empty() {
                let t = picker.custom.trim();
                if t.is_empty() { Vec::new() } else { vec![t.to_string()] }
            } else {
                picker.comps.iter().enumerate().filter(|(i, _)| picker.selected.get(*i).copied().unwrap_or(false)).map(|(_, n)| n.clone()).collect()
            };
            if picker.comps.is_empty() || !comps.is_empty() {
                match self.handle.lock(|s| s.add_effectcraft_comps(picker.path.clone(), &comps, &preset)) {
                    Ok(ids) => {
                        self.handle.note_change();
                        self.handle.schedule_probe(ids.clone());
                        self.status = format!("Queued {} EffectCraft comp(s)", ids.len());
                        queued_ok = true;
                    }
                    Err(e) => self.set_drop_notice(format!("Could not queue EffectCraft project: {e}")),
                }
            } else {
                self.set_drop_notice("Select at least one composition, or Cancel.");
            }
        }
        if open && !cancel && !queued_ok {
            self.ecproj_picker = Some(picker);
        }
    }

    pub(crate) fn watch_panel(&mut self, ui: &mut Ui) {
        ui.label("New files dropped into a watched folder are queued with the chosen preset.");
        let mut folders = self.handle.lock(|s| s.settings.watch_folders.clone());
        let mut remove = None;
        for (i, w) in folders.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                ui.checkbox(&mut w.enabled, "");
                ui.label(RichText::new(w.path.display().to_string()).color(self.t().link));
                ui.add(egui::TextEdit::singleline(&mut w.preset_id).desired_width(160.0));
                if ui.button("Remove").clicked() {
                    remove = Some(i);
                }
            });
        }
        if let Some(i) = remove {
            folders.remove(i);
        }
        if ui.button("Add folder…").clicked() {
            if let Some(p) = rfd::FileDialog::new().pick_folder() {
                let preset = self.handle.lock(|s| s.settings.default_preset_id.clone());
                folders.push(encodecraft_engine::WatchFolder::new(p, preset));
            }
        }
        if folders.is_empty() {
            ui.label(RichText::new("No folders yet. Add a folder to watch, or drop a directory onto the queue.").color(self.t().text_dim));
        }
        self.handle.lock(|s| s.settings.watch_folders = folders);
    }

    pub(crate) fn pick_sources(&mut self) {
        let files = rfd::FileDialog::new()
            .add_filter("Media", &["mp4", "mov", "mkv", "webm", "avi", "mxf", "mts", "m4v", "gif", "png", "jpg", "wav", "mp3", "aac", "m4a"])
            .add_filter("EffectCraft project", &["ecproj"])
            .pick_files()
            .unwrap_or_default();
        if files.is_empty() {
            return;
        }
        self.apply_drop_outcome(classify_dropped_paths(&files), 0);
    }

    fn import_preset(&mut self) {
        if let Some(p) = rfd::FileDialog::new().add_filter("Preset JSON", &["json"]).pick_file() {
            match self.handle.lock(|s| s.presets.import_preset(&p)) {
                Ok(pr) => self.status = format!("Imported {}", pr.name),
                Err(e) => self.status = e.to_string(),
            }
        }
    }

    fn export_preset(&mut self) {
        let id = self.handle.lock(|s| s.settings.default_preset_id.clone());
        if let Some(p) = rfd::FileDialog::new().set_file_name(format!("{id}.json")).save_file() {
            match self.handle.lock(|s| s.presets.export_preset(&id, &p)) {
                Ok(()) => self.status = "Preset exported".into(),
                Err(e) => self.status = e.to_string(),
            }
        }
    }

    fn ensure_thumb(&mut self, _ctx: &egui::Context, job: &Job) -> Option<&egui::TextureHandle> {
        if self.thumb_tex.contains_key(&job.id) {
            return self.thumb_tex.get(&job.id);
        }
        if self.thumb_inflight.insert(job.id.clone()) {
            self.handle.submit_io(IoCmd::Thumb(job.id.clone()));
        }
        None
    }
}

fn selected_job(h: &Handle) -> Option<Job> {
    h.lock(|s| s.selected.as_ref().and_then(|id| s.queue.jobs.iter().find(|j| j.id == *id)).cloned())
}

pub(crate) fn status_color(st: JobStatus, t: &Tokens) -> Color32 {
    match st {
        JobStatus::Waiting => t.text_dim,
        JobStatus::Encoding => t.text,
        JobStatus::Paused => t.warning,
        JobStatus::Done => t.cache_green,
        JobStatus::Failed => t.danger,
        JobStatus::Stopped => t.text_faint,
    }
}

mod generated_icon {
    include!(concat!(env!("OUT_DIR"), "/app_icon.rs"));
}

fn window_icon() -> Option<egui::IconData> {
    let bytes = generated_icon::png_bytes()?;
    let img = image::load_from_memory(bytes).ok()?.into_rgba8();
    let (width, height) = img.dimensions();
    Some(egui::IconData { rgba: img.into_raw(), width, height })
}

/// Native window options shared by the desktop app.
pub fn native_options() -> eframe::NativeOptions {
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([1440.0, 900.0])
        .with_min_inner_size([960.0, 620.0])
        .with_title(PRODUCT)
        .with_drag_and_drop(true);
    if let Some(icon) = window_icon() {
        viewport = viewport.with_icon(icon);
    }
    eframe::NativeOptions { viewport, ..Default::default() }
}

pub fn run(handle: Handle) -> eframe::Result<()> {
    eframe::run_native(PRODUCT, native_options(), Box::new(|_cc| Ok(Box::new(EncodeApp::new(handle)))))
}

#[cfg(test)]
mod tests {
    #[test]
    fn virtualized_list_draws_only_visible_rows() {
        let ctx = egui::Context::default();
        let mut drawn = 0usize;
        let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 360.0))), ..Default::default() };
        let _ = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                egui::ScrollArea::vertical().show_rows(ui, 36.0, 1000, |ui, range| {
                    drawn = range.len();
                    for i in range {
                        ui.label(format!("row {i}"));
                    }
                });
            });
        });
        assert!(drawn > 0, "should draw the visible window");
        assert!(drawn < 40, "viewport must not build all 1,000 rows (drew {drawn})");
    }

    #[test]
    fn seed_1000_outputs_for_ui_snapshot() {
        let dir = std::env::temp_dir().join(format!("ec-bench-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        encodecraft_engine::set_home(dir);
        let session = encodecraft_engine::Handle::open_core().expect("core").lock(|s| {
            encodecraft_engine::seed_stress_queue(s, 1000).expect("seed");
            s.queue.jobs.len()
        });
        assert_eq!(session, 1000);
        let t0 = std::time::Instant::now();
        let n = encodecraft_engine::Handle::open_core().expect("core2");
        // reuse already-seeded? new session empty — time for_ui clone of 1000
        n.lock(|s| {
            encodecraft_engine::seed_stress_queue(s, 1000).expect("seed2");
            let _views: Vec<_> = s.queue.jobs.iter().map(encodecraft_queue::Job::for_ui).collect();
        });
        let ms = t0.elapsed().as_secs_f64() * 1000.0;
        eprintln!("1000-row for_ui snapshot: {ms:.2} ms");
        assert!(ms < 200.0, "snapshot of 1,000 rows took {ms:.1} ms");
        let _ = session;
    }

    #[test]
    fn window_uses_frame_e_cobalt_icon() {
        let icon = super::window_icon();
        assert!(icon.is_some(), "encodecraft-256.png must be embedded as the window icon");
        if let Some(icon) = icon {
            assert_eq!(icon.width, 256);
            assert_eq!(icon.height, 256);
            assert_eq!(icon.rgba.len(), 256 * 256 * 4);
        }
        let _ = super::native_options();
    }
}
