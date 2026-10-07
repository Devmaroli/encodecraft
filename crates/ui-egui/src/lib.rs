//! EncodeCraft desktop UI — Media Encoder-style queue, preset browser, encoding panel.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod theme;

use std::collections::HashMap;
use std::path::PathBuf;

use egui::{Color32, RichText, Sense, Ui, Vec2};
use encodecraft_engine::Handle;
use encodecraft_job::{ScaleMode, ScreenSize, PRODUCT};
use encodecraft_presets::{ExportPreset, FrameRate, OutputFormat, PresetKind, Quality};
use encodecraft_queue::{Job, JobStatus};

use crate::theme::{ThemeKind, Tokens};

pub struct EncodeApp {
    handle: Handle,
    tokens: Tokens,
    bottom_tab: BottomTab,
    preset_filter: String,
    log_filter: String,
    show_watch: bool,
    show_batch: bool,
    show_preset_edit: bool,
    show_about: bool,
    batch_sizes: Vec<bool>,
    edit_preset: Option<ExportPreset>,
    thumb_tex: HashMap<String, egui::TextureHandle>,
    status: String,
    last_persist: std::time::Instant,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BottomTab {
    Encode,
    Preview,
    Log,
    Settings,
}

impl EncodeApp {
    pub fn new(handle: Handle) -> Self {
        Self {
            handle,
            tokens: Tokens::for_kind(ThemeKind::Dark),
            bottom_tab: BottomTab::Encode,
            preset_filter: String::new(),
            log_filter: String::new(),
            show_watch: false,
            show_batch: false,
            show_preset_edit: false,
            show_about: false,
            batch_sizes: Vec::new(),
            edit_preset: None,
            thumb_tex: HashMap::new(),
            status: String::new(),
            last_persist: std::time::Instant::now(),
        }
    }

    fn t(&self) -> Tokens {
        self.tokens
    }
}

impl eframe::App for EncodeApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        theme::apply(ctx, &self.tokens);
        ctx.request_repaint_after(std::time::Duration::from_millis(200));

        self.ingest_drops(ctx);
        if self.last_persist.elapsed() > std::time::Duration::from_secs(4) {
            self.handle.persist();
            self.last_persist = std::time::Instant::now();
        }

        self.menu(ctx);
        self.toolbar(ctx);
        let t = self.t();
        egui::CentralPanel::default().frame(egui::Frame::NONE.fill(t.app_bg)).show(ctx, |ui| {
            let avail = ui.available_size();
            let bottom_h = (avail.y * 0.34).clamp(180.0, 320.0);
            let top_h = avail.y - bottom_h - 4.0;
            ui.allocate_ui(Vec2::new(avail.x, top_h), |ui| {
                egui::SidePanel::right("presets")
                    .resizable(true)
                    .default_width(280.0)
                    .width_range(200.0..=420.0)
                    .frame(egui::Frame::NONE.fill(t.panel_bg).inner_margin(8.0))
                    .show_inside(ui, |ui| self.preset_browser(ui));
                egui::CentralPanel::default().frame(egui::Frame::NONE.fill(t.app_bg).inner_margin(6.0)).show_inside(ui, |ui| self.queue_panel(ui));
            });
            ui.separator();
            ui.allocate_ui(Vec2::new(avail.x, bottom_h), |ui| self.bottom(ui));
        });
        self.ffmpeg_banner(ctx);
        self.dialogs(ctx);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.handle.persist();
        encodecraft_engine::clear_lock();
    }
}

impl EncodeApp {
    fn ingest_drops(&mut self, ctx: &egui::Context) {
        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().filter_map(|f| f.path.clone()).collect());
        if dropped.is_empty() {
            return;
        }
        let preset = self.handle.lock(|s| s.settings.default_preset_id.clone());
        let _ = self.handle.lock(|s| s.add_files(&dropped, &preset));
        self.status = format!("Added {} source(s)", dropped.len());
    }

    fn menu(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("menu").exact_height(28.0).show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.menu_button("File", |ui| {
                    if ui.button("Add source…").clicked() {
                        self.pick_sources();
                        ui.close();
                    }
                    if ui.button("One source → many sizes…").clicked() {
                        self.show_batch = true;
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
                        self.handle.lock(|s| s.queue.running = true);
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
                });
                ui.menu_button("Watch", |ui| {
                    if ui.button("Watch folders…").clicked() {
                        self.show_watch = true;
                        ui.close();
                    }
                });
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
                    let running = self.handle.lock(|s| s.queue.running);
                    let start = egui::Button::new(if running { "  Queue running  " } else { "  ▶  Start queue  " });
                    if ui.add(start.fill(if running { t.cache_green } else { t.accent })).clicked() {
                        self.handle.lock(|s| s.queue.running = true);
                    }
                    if ui.button("  ⏸  Pause  ").clicked() {
                        self.handle.lock(|s| s.queue.running = false);
                    }
                    if ui.button("  ⏹  Stop  ").clicked() {
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
                            }
                        }
                    }
                    if ui.button("Remove").clicked() {
                        if let Some(id) = self.handle.lock(|s| s.selected.clone()) {
                            let _ = self.handle.lock(|s| s.queue.remove(&id));
                        }
                    }
                    if ui.button("↑").clicked() {
                        if let Some(id) = self.handle.lock(|s| s.selected.clone()) {
                            let _ = self.handle.lock(|s| s.queue.move_job(&id, -1));
                        }
                    }
                    if ui.button("↓").clicked() {
                        if let Some(id) = self.handle.lock(|s| s.selected.clone()) {
                            let _ = self.handle.lock(|s| s.queue.move_job(&id, 1));
                        }
                    }
                    ui.separator();
                    if ui.button("One → many sizes").clicked() {
                        self.show_batch = true;
                    }
                    ui.separator();
                    ui.label("Parallel");
                    let mut n = self.handle.lock(|s| s.settings.max_parallel as i32);
                    if ui.add(egui::DragValue::new(&mut n).range(1..=8)).changed() {
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

    fn queue_panel(&mut self, ui: &mut Ui) {
        let t = self.t();
        ui.horizontal(|ui| {
            ui.label(RichText::new("Queue").strong().size(14.0));
            let (n, run) = self.handle.lock(|s| (s.queue.jobs.len(), s.queue.running));
            ui.label(RichText::new(format!("{n} item(s) · {}", if run { "running" } else { "idle" })).color(t.text_dim));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(RichText::new("Drop media here").color(t.text_faint).italics().size(12.0));
            });
        });
        ui.add_space(4.0);
        let jobs: Vec<Job> = self.handle.lock(|s| s.queue.jobs.clone());
        let selected = self.handle.lock(|s| s.selected.clone());
        if jobs.is_empty() {
            ui.vertical_centered(|ui| {
                ui.add_space(48.0);
                ui.label(RichText::new("Queue is empty").size(16.0).color(t.text_dim));
                ui.label(RichText::new("Add a source, drop files, or send a job from EffectCraft.").color(t.text_faint));
                ui.add_space(8.0);
                if ui.button("Add source…").clicked() {
                    self.pick_sources();
                }
            });
            return;
        }
        egui::ScrollArea::vertical().id_salt("queue-scroll").show(ui, |ui| {
            for (i, job) in jobs.iter().enumerate() {
                let is_sel = selected.as_deref() == Some(job.id.as_str());
                let bg = if is_sel {
                    t.row_selected
                } else if i % 2 == 0 {
                    t.row
                } else {
                    t.row_alt
                };
                let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 56.0), Sense::click());
                ui.painter().rect_filled(rect, t.radius_sm, bg);
                if resp.clicked() {
                    self.handle.lock(|s| s.selected = Some(job.id.clone()));
                }
                if resp.double_clicked() {
                    self.bottom_tab = BottomTab::Encode;
                }
                let mut child =
                    ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink2(Vec2::new(10.0, 6.0))).layout(egui::Layout::left_to_right(egui::Align::Center)));
                child.horizontal(|ui| {
                    status_dot(ui, job.status, &t);
                    ui.vertical(|ui| {
                        ui.label(RichText::new(job.source.display_name()).strong());
                        let size = job.screen.as_ref().map(|s| s.label()).unwrap_or_else(|| match (job.preset.video.width, job.preset.video.height) {
                            (Some(w), Some(h)) => format!("{w}×{h}"),
                            _ => "Match source".into(),
                        });
                        ui.label(
                            RichText::new(format!(
                                "{}  ·  {size}  ·  {}",
                                job.preset.name,
                                job.output.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
                            ))
                            .size(11.0)
                            .color(t.text_dim),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.vertical(|ui| {
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                                ui.label(RichText::new(job.status.label()).color(status_color(job.status, &t)).size(12.0));
                            });
                            if job.status == JobStatus::Encoding || job.progress.percent > 0.0 {
                                let p = (job.progress.percent / 100.0).clamp(0.0, 1.0);
                                ui.add(egui::ProgressBar::new(p).desired_width(180.0).text(format!(
                                    "{:.0}%  {}  ETA {}",
                                    job.progress.percent,
                                    speed_lbl(job.progress.speed),
                                    eta_lbl(job.progress.eta_sec)
                                )));
                            } else if let Some(err) = &job.error {
                                ui.label(RichText::new(err).color(t.danger).size(11.0));
                            }
                        });
                    });
                });
            }
        });
    }

    fn preset_browser(&mut self, ui: &mut Ui) {
        let t = self.t();
        ui.label(RichText::new("Preset browser").strong().size(14.0));
        ui.add(egui::TextEdit::singleline(&mut self.preset_filter).hint_text("Filter presets"));
        ui.add_space(4.0);
        let filter = self.preset_filter.to_ascii_lowercase();
        let (presets, screens) = self.handle.lock(|s| (s.presets.all(), s.presets.screens.clone()));
        egui::ScrollArea::vertical().id_salt("preset-scroll").show(ui, |ui| {
            ui.label(RichText::new("System").small().color(t.text_faint));
            for p in presets.iter().filter(|p| p.kind == PresetKind::System) {
                if !filter.is_empty() && !p.name.to_ascii_lowercase().contains(&filter) {
                    continue;
                }
                self.preset_row(ui, p);
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("User").small().color(t.text_faint));
                if ui.small_button("+").clicked() {
                    if let Ok(dup) = self.handle.lock(|s| s.presets.duplicate(&s.settings.default_preset_id.clone(), "Custom preset")) {
                        self.edit_preset = Some(dup);
                        self.show_preset_edit = true;
                    }
                }
            });
            for p in presets.iter().filter(|p| p.kind == PresetKind::User) {
                if !filter.is_empty() && !p.name.to_ascii_lowercase().contains(&filter) {
                    continue;
                }
                self.preset_row(ui, p);
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("Screen sizes (DOOH)").small().color(t.text_faint));
                if ui.small_button("+").clicked() {
                    self.handle.lock(|s| s.presets.upsert_screen(ScreenSize::new("Custom", 1080, 1080)));
                }
            });
            for sz in &screens {
                let label = sz.label();
                if !filter.is_empty() && !label.to_ascii_lowercase().contains(&filter) {
                    continue;
                }
                ui.horizontal(|ui| {
                    if ui.selectable_label(false, &label).clicked() {
                        if let Some(id) = self.handle.lock(|s| s.selected.clone()) {
                            self.handle.lock(|s| {
                                if let Some(j) = s.queue.jobs.iter_mut().find(|j| j.id == id) {
                                    j.preset.video.width = Some(sz.width);
                                    j.preset.video.height = Some(sz.height);
                                    j.screen = Some(sz.clone());
                                }
                            });
                        }
                    }
                });
            }
        });
    }

    fn preset_row(&mut self, ui: &mut Ui, p: &ExportPreset) {
        ui.horizontal(|ui| {
            let resp = ui.selectable_label(false, format!("{}  ({})", p.name, p.format.extension()));
            if resp.clicked() {
                let id = p.id.clone();
                self.handle.lock(|s| s.settings.default_preset_id = id.clone());
                if let Some(sel) = s_selected(&self.handle) {
                    self.handle.lock(|s| {
                        if let Ok(pr) = s.presets.get(&id) {
                            if let Some(j) = s.queue.jobs.iter_mut().find(|j| j.id == sel) {
                                let w = j.preset.video.width;
                                let h = j.preset.video.height;
                                j.preset = pr;
                                if let (Some(w), Some(h)) = (w, h) {
                                    j.preset.video.width = Some(w);
                                    j.preset.video.height = Some(h);
                                }
                            }
                        }
                    });
                }
            }
            if resp.double_clicked() {
                self.edit_preset = Some(p.clone());
                self.show_preset_edit = true;
            }
        });
    }

    fn bottom(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.bottom_tab, BottomTab::Encode, "Encoding");
            ui.selectable_value(&mut self.bottom_tab, BottomTab::Preview, "Source");
            ui.selectable_value(&mut self.bottom_tab, BottomTab::Log, "Log");
            ui.selectable_value(&mut self.bottom_tab, BottomTab::Settings, "Settings");
        });
        ui.separator();
        match self.bottom_tab {
            BottomTab::Encode => self.encode_panel(ui),
            BottomTab::Preview => self.preview_panel(ui),
            BottomTab::Log => self.log_panel(ui),
            BottomTab::Settings => self.settings_panel(ui),
        }
    }

    fn encode_panel(&mut self, ui: &mut Ui) {
        let t = self.t();
        let Some(mut job) = selected_job(&self.handle) else {
            ui.label(RichText::new("Select a queue item to edit export settings.").color(t.text_dim));
            return;
        };
        ui.horizontal(|ui| {
            ui.label("Preset");
            ui.label(RichText::new(&job.preset.name).strong());
            ui.separator();
            ui.label("Output");
            ui.add(egui::Label::new(job.output.to_string_lossy().to_string()).truncate());
        });
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
            if ui.add(egui::DragValue::new(&mut w).range(0..=7680).suffix(" px")).changed() {
                job.preset.video.width = if w == 0 { None } else { Some(w as u32) };
            }
            ui.label("Height");
            let mut h = job.preset.video.height.unwrap_or(0) as i32;
            if ui.add(egui::DragValue::new(&mut h).range(0..=4320).suffix(" px")).changed() {
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
            }
            if ui.button("CRF / bitrate").clicked() {
                job.preset.video.quality = match job.preset.video.quality {
                    Quality::Crf { .. } => Quality::bitrate(5000),
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
        self.handle.lock(|s| {
            if let Some(j) = s.queue.jobs.iter_mut().find(|j| j.id == id) {
                *j = job;
            }
        });
    }

    fn preview_panel(&mut self, ui: &mut Ui) {
        let t = self.t();
        let Some(job) = selected_job(&self.handle) else {
            ui.label(RichText::new("Select a job to preview the source.").color(t.text_dim));
            return;
        };
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.label(RichText::new(job.source.display_name()).strong());
                ui.label(job.source.path_hint().display().to_string());
                if let Some(info) = self.handle.lock(|s| s.probe_job(&job)) {
                    ui.label(info.summary());
                }
                ui.label(format!("→ {}", job.output.display()));
            });
            ui.add_space(16.0);
            if let Some(tex) = self.ensure_thumb(ui.ctx(), &job) {
                ui.image((tex.id(), Vec2::new(240.0, 135.0)));
            } else {
                ui.allocate_ui(Vec2::new(240.0, 135.0), |ui| {
                    ui.centered_and_justified(|ui| ui.label(RichText::new("No thumbnail").color(t.text_faint)));
                });
            }
        });
    }

    fn log_panel(&mut self, ui: &mut Ui) {
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
        egui::ScrollArea::vertical().id_salt("log").stick_to_bottom(true).show(ui, |ui| {
            ui.add(egui::TextEdit::multiline(&mut dummy(&shown)).font(egui::TextStyle::Monospace).desired_width(f32::INFINITY));
        });
    }

    fn settings_panel(&mut self, ui: &mut Ui) {
        let mut s = self.handle.lock(|x| x.settings.clone());
        ui.label(RichText::new("Defaults").strong());
        ui.horizontal(|ui| {
            ui.label("Output folder");
            let mut out = s.output_dir.to_string_lossy().into_owned();
            if ui.add(egui::TextEdit::singleline(&mut out).desired_width(400.0)).changed() {
                s.output_dir = PathBuf::from(out);
            }
            if ui.button("Browse…").clicked() {
                if let Some(d) = rfd::FileDialog::new().pick_folder() {
                    s.output_dir = d;
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
            ui.add(egui::TextEdit::singleline(&mut s.default_preset_id).desired_width(220.0));
        });
        ui.horizontal(|ui| {
            ui.label("Control port");
            ui.add(egui::DragValue::new(&mut s.http_port).range(1024..=65535));
            ui.label(RichText::new("EffectCraft posts to 127.0.0.1:this").small());
        });
        ui.checkbox(&mut s.auto_start_queue, "Start the queue when a job arrives from EffectCraft / the CLI");
        if ui.button("Apply settings").clicked() {
            self.handle.lock(|sess| sess.settings = s.clone());
            self.handle.persist();
            self.status = "Settings saved".into();
        }
    }

    fn ffmpeg_banner(&self, ctx: &egui::Context) {
        let (ok, msg) = self.handle.lock(|s| match &s.ffmpeg {
            Ok(f) => (true, f.ok_message()),
            Err(e) => (false, e.to_string()),
        });
        if ok {
            return;
        }
        let t = self.t();
        egui::TopBottomPanel::bottom("ff-missing").frame(egui::Frame::NONE.fill(Color32::from_rgb(80, 24, 20)).inner_margin(8.0)).show(ctx, |ui| {
            ui.label(RichText::new("FFmpeg not found").strong().color(Color32::WHITE));
            ui.label(RichText::new(msg).color(t.tab_text_active).size(12.0));
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
        if self.show_watch {
            self.watch_dialog(ctx);
        }
        if self.show_batch {
            self.batch_dialog(ctx);
        }
        if self.show_preset_edit {
            self.preset_edit_dialog(ctx);
        }
    }

    fn watch_dialog(&mut self, ctx: &egui::Context) {
        let mut open = self.show_watch;
        egui::Window::new("Watch folders").open(&mut open).resizable(true).default_width(520.0).show(ctx, |ui| {
            ui.label("New files dropped into a watched folder are queued with the chosen preset.");
            let mut folders = self.handle.lock(|s| s.settings.watch_folders.clone());
            let mut remove = None;
            for (i, w) in folders.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    ui.checkbox(&mut w.enabled, "");
                    ui.label(w.path.display().to_string());
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
            self.handle.lock(|s| s.settings.watch_folders = folders);
        });
        self.show_watch = open;
    }

    fn batch_dialog(&mut self, ctx: &egui::Context) {
        let mut open = self.show_batch;
        let screens = self.handle.lock(|s| s.presets.screens.clone());
        if self.batch_sizes.len() != screens.len() {
            self.batch_sizes = screens.iter().map(|s| matches!((s.width, s.height), (1920, 1080) | (1080, 1920) | (1200, 960) | (1080, 864))).collect();
        }
        let mut close = false;
        egui::Window::new("One source → many sizes").open(&mut open).default_width(420.0).show(ctx, |ui| {
            ui.label("Pick sources and the DOOH sizes to emit. Each size becomes its own queue item.");
            for (i, sz) in screens.iter().enumerate() {
                if let Some(flag) = self.batch_sizes.get_mut(i) {
                    ui.checkbox(flag, sz.label());
                }
            }
            if ui.button("Choose files and add to queue").clicked() {
                let files =
                    rfd::FileDialog::new().add_filter("Media", &["mp4", "mov", "mkv", "webm", "avi", "mxf", "wav", "mp3"]).pick_files().unwrap_or_default();
                let sizes: Vec<ScreenSize> =
                    screens.iter().enumerate().filter(|(i, _)| self.batch_sizes.get(*i).copied().unwrap_or(false)).map(|(_, s)| s.clone()).collect();
                if !files.is_empty() && !sizes.is_empty() {
                    let preset = self.handle.lock(|s| s.settings.default_preset_id.clone());
                    match self.handle.lock(|s| s.add_files_to_sizes(&files, &preset, sizes)) {
                        Ok(ids) => self.status = format!("Queued {} job(s)", ids.len()),
                        Err(e) => self.status = e.to_string(),
                    }
                    close = true;
                }
            }
        });
        self.show_batch = open && !close;
    }

    fn preset_edit_dialog(&mut self, ctx: &egui::Context) {
        let mut open = self.show_preset_edit;
        let mut save = false;
        if let Some(p) = self.edit_preset.as_mut() {
            egui::Window::new("Edit preset").open(&mut open).show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Name");
                    ui.text_edit_singleline(&mut p.name);
                });
                ui.label(format!("id  {}", p.id));
                ui.label(p.format.label());
                if ui.button("Save user preset").clicked() {
                    save = true;
                }
            });
        }
        if save {
            if let Some(p) = self.edit_preset.clone() {
                let _ = self.handle.lock(|s| s.presets.upsert_user(p));
                self.handle.persist();
            }
            open = false;
        }
        self.show_preset_edit = open;
    }

    fn pick_sources(&mut self) {
        let files = rfd::FileDialog::new()
            .add_filter("Media", &["mp4", "mov", "mkv", "webm", "avi", "mxf", "mts", "m4v", "gif", "png", "jpg", "wav", "mp3", "aac", "m4a"])
            .pick_files()
            .unwrap_or_default();
        if files.is_empty() {
            return;
        }
        let preset = self.handle.lock(|s| s.settings.default_preset_id.clone());
        match self.handle.lock(|s| s.add_files(&files, &preset)) {
            Ok(ids) => self.status = format!("Added {} source(s)", ids.len()),
            Err(e) => self.status = e.to_string(),
        }
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

    fn ensure_thumb(&mut self, ctx: &egui::Context, job: &Job) -> Option<&egui::TextureHandle> {
        if self.thumb_tex.contains_key(&job.id) {
            return self.thumb_tex.get(&job.id);
        }
        let path = self.handle.lock(|s| s.thumbnail(job))?;
        let bytes = std::fs::read(&path).ok()?;
        let img = image::load_from_memory(&bytes).ok()?.to_rgba8();
        let size = [img.width() as usize, img.height() as usize];
        let color = egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw());
        let tex = ctx.load_texture(format!("th-{}", job.id), color, egui::TextureOptions::LINEAR);
        self.thumb_tex.insert(job.id.clone(), tex);
        self.thumb_tex.get(&job.id)
    }
}

fn selected_job(h: &Handle) -> Option<Job> {
    h.lock(|s| s.selected.as_ref().and_then(|id| s.queue.jobs.iter().find(|j| j.id == *id)).cloned())
}

fn s_selected(h: &Handle) -> Option<String> {
    h.lock(|s| s.selected.clone())
}

fn dummy(s: &str) -> String {
    s.to_string()
}

fn status_dot(ui: &mut Ui, st: JobStatus, t: &Tokens) {
    let c = status_color(st, t);
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
    ui.painter().circle_filled(rect.center(), 4.5, c);
}

fn status_color(st: JobStatus, t: &Tokens) -> Color32 {
    match st {
        JobStatus::Waiting => t.text_dim,
        JobStatus::Encoding => t.accent,
        JobStatus::Paused => t.warning,
        JobStatus::Done => t.cache_green,
        JobStatus::Failed => t.danger,
        JobStatus::Stopped => t.text_faint,
    }
}

fn speed_lbl(s: f32) -> String {
    if s <= 0.0 {
        "—".into()
    } else {
        format!("{s:.2}×")
    }
}

fn eta_lbl(s: Option<f64>) -> String {
    match s {
        Some(v) if v.is_finite() => encodecraft_ffmpeg::format_duration(v),
        _ => "—".into(),
    }
}

/// Native window options shared by the desktop app.
pub fn native_options() -> eframe::NativeOptions {
    eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1440.0, 900.0])
            .with_min_inner_size([960.0, 620.0])
            .with_title(PRODUCT)
            .with_drag_and_drop(true),
        ..Default::default()
    }
}

pub fn run(handle: Handle) -> eframe::Result<()> {
    eframe::run_native(PRODUCT, native_options(), Box::new(|_cc| Ok(Box::new(EncodeApp::new(handle)))))
}
