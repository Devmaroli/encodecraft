//! Queue / Preset Browser / settings windows matching the approved mockups.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use egui::{Align2, Color32, CornerRadius, CursorIcon, Id, LayerId, Order, Painter, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, Ui, Vec2};
use encodecraft_job::ScreenSize;
use encodecraft_presets::{
    ExportPreset, FrameRate, OutputFormat, PresetKind, PresetLibrary, PresetReason, Quality, ScreenMatchRule, BETTER_RES_ID, INSTALLATION_ID,
};
use encodecraft_queue::{Job, JobStatus};

use crate::theme::Tokens;
use crate::{BottomTab, EncodeApp};

#[derive(Clone)]
pub struct UndoToast {
    pub message: String,
    pub snapshot: Vec<(String, ExportPreset, bool, Option<PresetReason>)>,
    pub order: Option<Vec<String>>,
    pub until: Instant,
}

/// Queue row / source-group drag payload (distinct from preset-id `String` drops).
#[derive(Clone, Debug)]
pub struct QueueDrag {
    pub ids: Vec<String>,
    pub source_key: String,
    pub is_source: bool,
    pub label: String,
}

#[derive(Clone)]
pub struct SourceGroup {
    pub key: String,
    pub title: String,
    pub subtitle: String,
    pub job_ids: Vec<String>,
}

impl EncodeApp {
    pub fn queue_panel_ux(&mut self, ui: &mut Ui) {
        let t = self.t();
        let jobs = std::mem::take(&mut self.jobs_cache);
        let groups = group_jobs(&jobs);
        let (better, inst) = studio_counts(&jobs);

        ui.horizontal(|ui| {
            ui.label(RichText::new("Queue").strong().size(14.0));
            let idle = !self.queue_running;
            ui.label(RichText::new(format!("{} item{} · {}", jobs.len(), if jobs.len() == 1 { "" } else { "s" }, if idle { "Idle" } else { "running" })).color(t.text_dim));
            if better + inst > 0 {
                ui.separator();
                studio_count_chip(ui, t.warning, &format!("{better} × Better Res"));
                studio_count_chip(ui, t.accent, &format!("{inst} × Installation"));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(RichText::new("Renders top to bottom · drag rows to change the order").color(t.text_faint).italics().size(12.0));
            });
        });
        ui.add_space(2.0);
        header_row(ui, &t);
        if jobs.is_empty() {
            ui.vertical_centered(|ui| {
                ui.add_space(40.0);
                ui.label(RichText::new("Queue is empty").size(16.0).color(t.text_dim));
                ui.label(RichText::new("Drop media, drop an EffectCraft .ecproj, or send a job from EffectCraft.").color(t.text_faint));
                if ui.button("Add source…").clicked() {
                    self.pick_sources();
                }
            });
            self.draw_toast(ui);
            self.jobs_cache = jobs;
            return;
        }

        let order: Vec<String> = groups.iter().flat_map(|g| g.job_ids.clone()).collect();
        let mut geoms: Vec<QueueGeom> = Vec::new();
        enum FlatRow<'a> {
            Source(&'a SourceGroup),
            Output(&'a Job),
        }
        let by_id: HashMap<&str, usize> = jobs.iter().enumerate().map(|(i, j)| (j.id.as_str(), i)).collect();
        let mut flat: Vec<FlatRow<'_>> = Vec::with_capacity(jobs.len().saturating_add(groups.len()));
        for g in &groups {
            flat.push(FlatRow::Source(g));
            if self.collapsed.contains(&g.key) {
                continue;
            }
            for id in &g.job_ids {
                if let Some(&idx) = by_id.get(id.as_str()) {
                    flat.push(FlatRow::Output(&jobs[idx]));
                }
            }
        }
        const ROW_H: f32 = 36.0;
        egui::ScrollArea::vertical().id_salt("queue-scroll").show_rows(ui, ROW_H, flat.len(), |ui, range| {
            for i in range {
                match flat[i] {
                    FlatRow::Source(g) => self.source_row(ui, g, &jobs, &order, &mut geoms),
                    FlatRow::Output(job) => self.output_row(ui, job, &order, &jobs, &mut geoms),
                }
            }
        });
        self.draw_toast(ui);
        self.paint_queue_drag_layer(ui, &jobs, &geoms);
        self.preset_popup(ui, &jobs);
        self.format_popup(ui, &jobs);
        self.jobs_cache = jobs;
    }

    fn source_row(&mut self, ui: &mut Ui, g: &SourceGroup, jobs: &[Job], _order: &[String], geoms: &mut Vec<QueueGeom>) {
        let t = self.t();
        let w = ui.available_width();
        let locked = group_has_locked(g, jobs);
        let sense = if locked { Sense::click() } else { Sense::click_and_drag() };
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, 28.0), sense);
        geoms.push(QueueGeom {
            rect,
            is_source: true,
            source_key: g.key.clone(),
            job_id: None,
            locked,
            job_ids: g.job_ids.clone(),
        });
        let dragging_self = egui::DragAndDrop::payload::<QueueDrag>(ui.ctx()).is_some_and(|d| d.is_source && d.source_key == g.key);
        ui.painter().rect_filled(rect, CornerRadius::ZERO, t.header_bg);
        if dragging_self {
            ui.painter().rect_filled(rect, CornerRadius::ZERO, Color32::from_rgba_unmultiplied(8, 10, 14, 170));
        }
        if !locked {
            let drag = QueueDrag { ids: g.job_ids.clone(), source_key: g.key.clone(), is_source: true, label: g.title.clone() };
            if resp.dragged() {
                egui::DragAndDrop::set_payload(ui.ctx(), drag);
                ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
            } else if resp.hovered() {
                ui.ctx().set_cursor_icon(CursorIcon::Grab);
            }
        } else if resp.hovered() {
            resp.clone().on_hover_text("Already rendering · can't be moved");
        }
        if let Some(pid) = resp.dnd_hover_payload::<String>() {
            ui.painter().rect_stroke(rect, CornerRadius::ZERO, egui::Stroke::new(1.5_f32, t.accent), egui::StrokeKind::Inside);
            let _ = pid;
        }
        if let Some(pid) = resp.dnd_release_payload::<String>() {
            if let Some(like) = g.job_ids.first() {
                match self.handle.lock(|s| s.add_output_with_preset(like, pid.as_str())) {
                    Ok(_) => {
                        self.status = format!("Added output with {}", pid);
                        self.handle.mark_dirty();
                    }
                    Err(e) => self.status = e.to_string(),
                }
            }
        }
        if resp.clicked() && !resp.dragged() {
            if self.collapsed.contains(&g.key) {
                self.collapsed.remove(&g.key);
            } else {
                self.collapsed.insert(g.key.clone());
            }
        }
        let collapsed = self.collapsed.contains(&g.key);
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink2(Vec2::new(8.0, 4.0))).layout(egui::Layout::left_to_right(egui::Align::Center)));
        child.horizontal(|ui| {
            paint_grab_or_lock(ui, locked, &t);
            ui.label(RichText::new(if collapsed { ">" } else { "v" }).color(t.text_dim).strong());
            ui.label(RichText::new(&g.title).strong().color(t.tab_text_active));
            ui.label(RichText::new(&g.subtitle).color(t.text_dim).size(11.0));
            if resp.dnd_hover_payload::<String>().is_some() {
                ui.label(RichText::new("+ Add output").color(t.accent).size(11.0));
            }
        });
        resp.context_menu(|ui| {
            if ui.button("Add output to this source").clicked() {
                if let Some(like) = g.job_ids.first() {
                    let preset = self.browser_selected.clone().or_else(|| self.handle.lock(|s| s.settings.default_preset_id.clone()).into());
                    if let Some(pid) = preset {
                        let _ = self.handle.lock(|s| s.add_output_with_preset(like, &pid));
                    }
                }
                ui.close();
            }
            if ui.button("Remove group").clicked() {
                for id in &g.job_ids {
                    let _ = self.handle.lock(|s| s.queue.remove(id));
                }
                ui.close();
            }
        });
    }

    fn output_row(&mut self, ui: &mut Ui, job: &Job, order: &[String], all: &[Job], geoms: &mut Vec<QueueGeom>) {
        let t = self.t();
        let selected = self.selected_ids.contains(&job.id);
        let locked = job.status.is_order_locked();
        let src_key = job.source.group_key();
        let bg = if selected { t.row_selected } else { t.row };
        let w = ui.available_width();
        let sense = if locked { Sense::click() } else { Sense::click_and_drag() };
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, 36.0), sense);
        geoms.push(QueueGeom {
            rect,
            is_source: false,
            source_key: src_key.clone(),
            job_id: Some(job.id.clone()),
            locked,
            job_ids: vec![job.id.clone()],
        });
        ui.painter().rect_filled(rect, CornerRadius::ZERO, bg);
        if egui::DragAndDrop::payload::<QueueDrag>(ui.ctx()).is_some_and(|d| d.ids.iter().any(|id| *id == job.id)) {
            ui.painter().rect_filled(rect, CornerRadius::ZERO, Color32::from_rgba_unmultiplied(8, 10, 14, 170));
        }
        if !locked {
            let mut ids = vec![job.id.clone()];
            if selected && self.selected_ids.len() > 1 {
                ids = self.selected_ids.iter().filter(|id| all.iter().any(|j| j.id == **id && j.source.group_key() == src_key && !j.status.is_order_locked())).cloned().collect();
                if ids.is_empty() {
                    ids = vec![job.id.clone()];
                }
            }
            let name = job.source.display_name();
            let label = name.rsplit(" / ").next().unwrap_or(&name).to_string();
            let drag = QueueDrag { ids, source_key: src_key.clone(), is_source: false, label };
            if resp.dragged() {
                egui::DragAndDrop::set_payload(ui.ctx(), drag);
                ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
            } else if resp.hovered() {
                ui.ctx().set_cursor_icon(CursorIcon::Grab);
            }
        } else {
            let _ = resp.clone().on_hover_text("Already rendering · can't be moved");
        }
        if let Some(_) = resp.dnd_hover_payload::<String>() {
            ui.painter().rect_stroke(rect, CornerRadius::ZERO, egui::Stroke::new(1.5_f32, t.accent), egui::StrokeKind::Inside);
        }
        if let Some(pid) = resp.dnd_release_payload::<String>() {
            let targets = if selected && self.selected_ids.len() > 1 {
                self.selected_ids.clone()
            } else {
                vec![job.id.clone()]
            };
            self.apply_preset_ids(&targets, pid.as_str());
        }
        let mods = ui.input(|i| (i.modifiers.command || i.modifiers.ctrl, i.modifiers.shift));
        if resp.clicked() && !resp.dragged() {
            self.click_select(&job.id, order, mods.0, mods.1);
            self.handle.lock(|s| s.selected = Some(job.id.clone()));
        }
        if resp.double_clicked() {
            self.bottom_tab = BottomTab::Encode;
        }
        let cols = col_layout(rect);
        let mut src_ui = ui.new_child(egui::UiBuilder::new().max_rect(cols.src).layout(egui::Layout::left_to_right(egui::Align::Center)));
        src_ui.horizontal(|ui| {
            paint_grab_or_lock(ui, locked, &t);
            preset_dot(ui, &job.preset.id, &t);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                let name = job.source.display_name();
                let comp = name.rsplit(" / ").next().unwrap_or(&name);
                ui.label(RichText::new(comp).strong().size(12.5));
                let sub = job.screen.as_ref().map(|s| {
                    if s.width > 0 {
                        format!("{} · {}×{}", s.name, s.width, s.height)
                    } else {
                        s.name.clone()
                    }
                }).unwrap_or_else(|| match (job.preset.video.width, job.preset.video.height) {
                    (Some(w), Some(h)) => format!("Match source · {w}×{h}"),
                    _ => "Match source".into(),
                });
                ui.label(RichText::new(sub).size(10.5).color(t.text_dim));
            });
        });
        let mut fmt_ui = ui.new_child(egui::UiBuilder::new().max_rect(cols.fmt).layout(egui::Layout::left_to_right(egui::Align::Center)));
        fmt_ui.horizontal(|ui| {
            let r = ui.add(egui::Label::new(RichText::new(format!("{} v", job.preset.format.short_label())).color(t.accent).size(12.5)).sense(Sense::click()));
            if r.clicked() {
                self.format_popup_for = Some(job.id.clone());
                self.popup_anchor = Some(r.rect.left_bottom());
            }
        });
        let mut pr_ui = ui.new_child(egui::UiBuilder::new().max_rect(cols.preset).layout(egui::Layout::left_to_right(egui::Align::Center)));
        pr_ui.horizontal(|ui| {
            preset_dot(ui, &job.preset.id, &t);
            let r = ui.add(egui::Label::new(RichText::new(format!("{} v", job.preset.name)).color(t.accent).size(12.5)).sense(Sense::click()));
            if r.clicked() {
                self.preset_popup_for = Some(job.id.clone());
                self.preset_search.clear();
                self.popup_anchor = Some(r.rect.left_bottom());
            }
            assignment_tag(ui, job, &t);
            if job.preset_overridden {
                let reset = ui.add(egui::Label::new(RichText::new("↺ Auto").color(t.accent).size(11.0)).sense(Sense::click()));
                if reset.clicked() {
                    let _ = self.handle.lock(|s| s.reset_jobs_to_auto(&[job.id.clone()]));
                    self.handle.mark_dirty();
                }
            }
        });
        let mut out_ui = ui.new_child(egui::UiBuilder::new().max_rect(cols.out).layout(egui::Layout::left_to_right(egui::Align::Center)));
        let out_label = job.output.to_string_lossy().into_owned();
        let folder_tip = job.output.parent().map(|p| p.display().to_string()).unwrap_or_else(|| out_label.clone());
        let out_r = out_ui.add(
            egui::Label::new(RichText::new(out_label).size(11.0).color(t.accent))
                .truncate()
                .sense(Sense::click()),
        );
        let out_r = out_r.on_hover_cursor(CursorIcon::PointingHand).on_hover_text(format!("{folder_tip}\nOpen in File Explorer"));
        if out_r.hovered() {
            out_ui.painter().hline(out_r.rect.x_range(), out_r.rect.bottom() - 3.0, Stroke::new(1.0_f32, t.accent));
        }
        if out_r.clicked() {
            self.reveal_in_explorer(&job.output);
        }
        let mut st_ui = ui.new_child(egui::UiBuilder::new().max_rect(cols.status).layout(egui::Layout::left_to_right(egui::Align::Center)));
        st_ui.horizontal(|ui| {
            if job.metadata_pending {
                ui.add(egui::Spinner::new().size(11.0));
            }
            ui.label(RichText::new(if job.status == JobStatus::Waiting { "Ready" } else { job.status.label() }).size(11.5).color(crate::status_color(job.status, &t)));
        });

        let jid = job.id.clone();
        out_r.context_menu(|ui| {
            self.row_context_menu(ui, &jid, all);
        });
        resp.context_menu(|ui| {
            self.row_context_menu(ui, &jid, all);
        });
    }

    fn row_context_menu(&mut self, ui: &mut Ui, job_id: &str, all: &[Job]) {
        let presets = self.handle.lock(|s| s.presets.all());
        ui.menu_button("Apply preset", |ui| {
            for p in &presets {
                if ui.button(&p.name).clicked() {
                    self.apply_preset_ids(&[job_id.to_string()], &p.id);
                    ui.close();
                }
            }
        });
        ui.menu_button("Change format", |ui| {
            for f in OutputFormat::all() {
                if ui.button(f.label()).clicked() {
                    self.handle.lock(|s| {
                        if let Some(j) = s.queue.jobs.iter_mut().find(|j| j.id == job_id) {
                            j.preset.format = *f;
                        }
                    });
                    ui.close();
                }
            }
        });
        if ui.button("Change output file…").clicked() {
            if let Some(p) = rfd::FileDialog::new().save_file() {
                self.handle.lock(|s| {
                    if let Some(j) = s.queue.jobs.iter_mut().find(|j| j.id == job_id) {
                        j.output = p;
                    }
                });
            }
            ui.close();
        }
        if ui.button("Show in Explorer").clicked() {
            let path = self.handle.lock(|s| s.queue.jobs.iter().find(|j| j.id == job_id).map(|j| j.output.clone()));
            if let Some(p) = path {
                self.reveal_in_explorer(&p);
            }
            ui.close();
        }
        if ui.button("Duplicate").clicked() {
            if let Ok(n) = self.handle.lock(|s| s.queue.duplicate(job_id)) {
                self.handle.lock(|s| s.selected = Some(n));
            }
            ui.close();
        }
        if ui.button("Add output to this source").clicked() {
            let pid = self.browser_selected.clone().unwrap_or_else(|| self.handle.lock(|s| s.settings.default_preset_id.clone()));
            let _ = self.handle.lock(|s| s.add_output_with_preset(job_id, &pid));
            ui.close();
        }
        if ui.button("Reset preset to Auto").clicked() {
            let _ = self.handle.lock(|s| s.reset_jobs_to_auto(&[job_id.to_string()]));
            self.handle.mark_dirty();
            ui.close();
        }
        if ui.button("Remove").clicked() {
            let _ = self.handle.lock(|s| s.queue.remove(job_id));
            ui.close();
        }
        let _ = all;
    }

    fn click_select(&mut self, id: &str, order: &[String], ctrl: bool, shift: bool) {
        if shift {
            let anchor = self.shift_anchor.clone().unwrap_or_else(|| id.to_string());
            if let (Some(a), Some(b)) = (order.iter().position(|x| x == &anchor), order.iter().position(|x| x == id)) {
                let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
                self.selected_ids = order[lo..=hi].to_vec();
            }
            return;
        }
        self.shift_anchor = Some(id.to_string());
        if ctrl {
            if self.selected_ids.iter().any(|x| x == id) {
                self.selected_ids.retain(|x| x != id);
            } else {
                self.selected_ids.push(id.to_string());
            }
        } else {
            self.selected_ids = vec![id.to_string()];
        }
    }

    pub fn apply_preset_ids(&mut self, ids: &[String], preset_id: &str) {
        let snap = self.handle.lock(|s| s.snapshot_presets(ids));
        match self.handle.lock(|s| s.apply_preset_to_jobs(ids, preset_id)) {
            Ok(n) if n > 0 => {
                let name = self.handle.lock(|s| s.presets.get(preset_id).map(|p| p.name).unwrap_or_else(|_| preset_id.to_string()));
                self.toast = Some(UndoToast {
                    message: format!("{name} applied to {n} output{} — tags now say Manual", if n == 1 { "" } else { "s" }),
                    snapshot: snap,
                    order: None,
                    until: Instant::now() + Duration::from_secs(8),
                });
                self.handle.mark_dirty();
            }
            Ok(_) => {}
            Err(e) => self.status = e.to_string(),
        }
    }

    fn draw_toast(&mut self, ui: &mut Ui) {
        let Some(toast) = self.toast.clone() else {
            return;
        };
        if Instant::now() > toast.until {
            self.toast = None;
            return;
        }
        let can_undo = !toast.snapshot.is_empty() || toast.order.is_some();
        let fill = if can_undo { Color32::from_rgb(0x2a, 0x5a, 0x38) } else { Color32::from_rgb(0x3a, 0x38, 0x28) };
        let fg = if can_undo { Color32::from_rgb(0xd8, 0xf0, 0xdc) } else { Color32::from_rgb(0xf0, 0xe8, 0xd0) };
        let prefix = if can_undo { "✓  " } else { "" };
        egui::Frame::NONE.fill(fill).inner_margin(8.0).corner_radius(6.0).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("{prefix}{}", toast.message)).color(fg));
                if can_undo && ui.button("Undo (Ctrl+Z)").clicked() {
                    self.undo_toast();
                }
            });
        });
    }

    fn show_info_toast(&mut self, message: impl Into<String>) {
        self.toast = Some(UndoToast {
            message: message.into(),
            snapshot: Vec::new(),
            order: None,
            until: Instant::now() + Duration::from_secs(6),
        });
    }

    fn reveal_in_explorer(&mut self, path: &Path) {
        match plan_reveal(path) {
            Ok(plan) => {
                if let Err(e) = spawn_reveal(&plan) {
                    self.show_info_toast(format!("Couldn't open in Explorer ({e})"));
                }
            }
            Err(e) => self.show_info_toast(e),
        }
    }

    pub fn undo_toast(&mut self) {
        if let Some(t) = self.toast.take() {
            if let Some(order) = t.order {
                self.handle.lock(|s| s.queue.restore_order(&order));
                self.status = "Undid move".into();
            } else {
                self.handle.lock(|s| s.restore_preset_snapshot(&t.snapshot));
                self.status = "Undid preset apply".into();
            }
            self.handle.mark_dirty();
        }
    }

    fn apply_queue_reorder(&mut self, ids: &[String], dest: usize, label: &str) {
        if ids.is_empty() {
            return;
        }
        let before = self.handle.lock(|s| s.queue.job_ids());
        match self.handle.lock(|s| s.queue.reorder(ids, dest)) {
            Ok(pos) => {
                self.toast = Some(UndoToast {
                    message: if ids.len() > 1 {
                        format!("Moved {} items · {label} now #{pos}", ids.len())
                    } else {
                        format!("Moved {label} to #{pos}")
                    },
                    snapshot: Vec::new(),
                    order: Some(before),
                    until: Instant::now() + Duration::from_secs(8),
                });
                self.handle.mark_dirty();
            }
            Err(e) => self.status = e.to_string(),
        }
    }

    pub fn move_selected_rows(&mut self, delta: isize) {
        let ids = self.selected_ids.clone();
        if ids.is_empty() {
            return;
        }
        let dest = self.handle.lock(|s| {
            let min = ids.iter().filter_map(|id| s.queue.index_of(id)).min().unwrap_or(0);
            min.saturating_add_signed(delta)
        });
        let label = self.handle.lock(|s| {
            s.queue.jobs.iter().find(|j| j.id == ids[0]).map(|j| j.source.display_name()).unwrap_or_else(|| "rows".into())
        });
        self.apply_queue_reorder(&ids, dest, &label);
    }

    fn paint_queue_drag_layer(&mut self, ui: &mut Ui, jobs: &[Job], geoms: &[QueueGeom]) {
        let Some(drag) = egui::DragAndDrop::payload::<QueueDrag>(ui.ctx()).map(|d| (*d).clone()) else {
            return;
        };
        let Some(pos) = ui.ctx().pointer_interact_pos().or_else(|| ui.input(|i| i.pointer.hover_pos())) else {
            return;
        };
        let painter = ui.ctx().layer_painter(LayerId::new(Order::Foreground, Id::new("queue-drag-overlay")));
        let t = self.t();
        let released = ui.input(|i| i.pointer.any_released());
        if let Some(hit) = hit_geom(geoms, pos) {
            let top = pos.y < hit.rect.center().y;
            if drag.is_source {
                let dest = if hit.is_source {
                    drop_dest_for_source(jobs, &SourceGroup {
                        key: hit.source_key.clone(),
                        title: String::new(),
                        subtitle: String::new(),
                        job_ids: hit.job_ids.clone(),
                    }, &drag.ids, top)
                } else {
                    group_jobs(jobs)
                        .iter()
                        .find(|g| g.key == hit.source_key)
                        .map(|g| drop_dest_for_source(jobs, g, &drag.ids, top))
                        .unwrap_or(0)
                };
                paint_insert_line(&painter, hit.rect, top, t.accent, dest);
                if released {
                    self.apply_queue_reorder(&drag.ids, dest, &drag.label);
                }
            } else if drag.source_key != hit.source_key || hit.is_source {
                paint_forbidden(&painter, hit.rect, "Outputs stay with their own source");
                if released {
                    self.show_info_toast("Outputs stay with their own source");
                }
            } else if hit.locked && top {
                paint_forbidden(&painter, hit.rect, "Already rendering · can't be moved");
            } else if let Some(id) = &hit.job_id {
                let dest = drop_dest_for_output(jobs, id, &drag.ids, top);
                paint_insert_line(&painter, hit.rect, top, t.accent, dest);
                if released {
                    self.apply_queue_reorder(&drag.ids, dest, &drag.label);
                }
            }
        }
        let ghost = Rect::from_min_size(pos + Vec2::new(14.0, 10.0), Vec2::new(420.0, 36.0));
        painter.rect_filled(ghost, CornerRadius::same(6), Color32::from_rgba_unmultiplied(0x1a, 0x2a, 0x40, 200));
        painter.rect_stroke(ghost, CornerRadius::same(6), Stroke::new(1.5_f32, t.accent), StrokeKind::Inside);
        let hx = ghost.left() + 12.0;
        let hy = ghost.center().y;
        for dy in [-4.0_f32, 0.0, 4.0] {
            painter.line_segment([Pos2::new(hx - 5.0, hy + dy), Pos2::new(hx + 5.0, hy + dy)], Stroke::new(1.4, Color32::from_rgb(0xb0, 0xc0, 0xd4)));
        }
        painter.text(
            ghost.left_center() + Vec2::new(24.0, 0.0),
            Align2::LEFT_CENTER,
            &drag.label,
            egui::FontId::proportional(13.0),
            Color32::WHITE,
        );
        if drag.ids.len() > 1 {
            let badge = Rect::from_center_size(ghost.right_center() - Vec2::new(44.0, 0.0), Vec2::new(64.0, 20.0));
            painter.rect_filled(badge, CornerRadius::same(10), Color32::from_rgb(0xc4, 0x3c, 0x4a));
            painter.text(badge.center(), Align2::CENTER_CENTER, format!("{} items", drag.ids.len()), egui::FontId::proportional(11.0), Color32::WHITE);
        }
    }

    fn preset_popup(&mut self, ui: &mut Ui, jobs: &[Job]) {
        let Some(job_id) = self.preset_popup_for.clone() else {
            return;
        };
        let Some(job) = jobs.iter().find(|j| j.id == job_id).cloned() else {
            self.preset_popup_for = None;
            return;
        };
        let pos = self.popup_anchor.unwrap_or(Pos2::new(200.0, 200.0));
        let mut open = true;
        let t = self.t();
        egui::Window::new("preset-pick")
            .id(Id::new("preset-pick-win"))
            .title_bar(false)
            .resizable(false)
            .collapsible(false)
            .fixed_pos(pos)
            .fixed_size(Vec2::new(420.0, 460.0))
            .open(&mut open)
            .show(ui.ctx(), |ui| {
                ui.add(egui::TextEdit::singleline(&mut self.preset_search).hint_text("Type to filter: \"better\", \"hevc\", \"6 Mbps\"…").desired_width(f32::INFINITY));
                let q = self.preset_search.to_ascii_lowercase();
                let (presets, recents, rules) = self.handle.lock(|s| (s.presets.all(), s.settings.recent_preset_ids.clone(), s.presets.screen_rules.clone()));
                let auto_pick = {
                    let hint = job.source.display_name();
                    let px = match (job.preset.video.width, job.preset.video.height) {
                        (Some(w), Some(h)) if w > 0 && h > 0 => Some((w, h)),
                        _ => None,
                    };
                    rules.pick(job.screen.as_ref(), &hint, px)
                };
                egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                    ui.label(RichText::new("STUDIO").small().color(t.text_faint).strong());
                    for p in presets.iter().filter(|p| PresetLibrary::is_studio_system(&p.id)) {
                        if !filter_preset(p, &q) {
                            continue;
                        }
                        let selected = p.id == job.preset.id;
                        if studio_pick_row(ui, p, selected, &t).clicked() {
                            self.apply_preset_ids(&[job_id.clone()], &p.id);
                            self.preset_popup_for = None;
                        }
                    }
                    ui.add_space(6.0);
                    if let Some(pick) = &auto_pick {
                        let name = presets.iter().find(|p| p.id == pick.preset_id).map(|p| p.name.clone()).unwrap_or_else(|| pick.preset_id.clone());
                        ui.label(RichText::new("Auto choice for this screen").small().color(t.text_faint));
                        let lab = format!("Auto choice for this screen: {name} ({})", pick.reason.label());
                        if ui.selectable_label(!job.preset_overridden, lab).clicked() {
                            let _ = self.handle.lock(|s| s.reset_jobs_to_auto(&[job_id.clone()]));
                            self.handle.mark_dirty();
                            self.preset_popup_for = None;
                        }
                    }
                    ui.add_space(6.0);
                    ui.label(RichText::new("RECENT").small().color(t.text_faint).strong());
                    for id in &recents {
                        if let Some(p) = presets.iter().find(|p| p.id == *id) {
                            if !filter_preset(p, &q) {
                                continue;
                            }
                            if ui.selectable_label(false, format!("{}   {}", p.name, if p.kind == PresetKind::System { "System" } else { "User" })).clicked() {
                                self.apply_preset_ids(&[job_id.clone()], &p.id);
                                self.preset_popup_for = None;
                            }
                        }
                    }
                    ui.add_space(6.0);
                    ui.label(RichText::new("ALL PRESETS").small().color(t.text_faint).strong());
                    ui.collapsing("System Presets", |ui| {
                        for p in presets.iter().filter(|p| p.kind == PresetKind::System && !PresetLibrary::is_studio_system(&p.id)) {
                            if filter_preset(p, &q) && ui.selectable_label(p.id == job.preset.id, &p.name).clicked() {
                                self.apply_preset_ids(&[job_id.clone()], &p.id);
                                self.preset_popup_for = None;
                            }
                        }
                    });
                    ui.collapsing("User Presets & Groups", |ui| {
                        for p in presets.iter().filter(|p| p.kind == PresetKind::User) {
                            if filter_preset(p, &q) && ui.selectable_label(p.id == job.preset.id, format!("{}  {}", p.name, if p.group.is_empty() { String::new() } else { format!("· {}", p.group) })).clicked() {
                                self.apply_preset_ids(&[job_id.clone()], &p.id);
                                self.preset_popup_for = None;
                            }
                        }
                    });
                });
                ui.separator();
                ui.horizontal(|ui| {
                    if ui.small_button("Edit preset settings…").clicked() {
                        if let Ok(p) = self.handle.lock(|s| s.presets.get(&job.preset.id)) {
                            self.edit_preset = Some(p);
                            self.show_preset_edit = true;
                        }
                        self.preset_popup_for = None;
                    }
                    if ui.small_button("Save these settings as a user preset…").clicked() {
                        self.save_job_as_user_preset(&job);
                        self.preset_popup_for = None;
                    }
                });
                ui.label(RichText::new("Tip: Ctrl / Shift-click several rows first to change them all at once.").small().color(t.text_faint));
            });
        if !open {
            self.preset_popup_for = None;
        }
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.preset_popup_for = None;
        }
    }

    fn format_popup(&mut self, ui: &mut Ui, jobs: &[Job]) {
        let Some(job_id) = self.format_popup_for.clone() else {
            return;
        };
        let Some(job) = jobs.iter().find(|j| j.id == job_id) else {
            self.format_popup_for = None;
            return;
        };
        let pos = self.popup_anchor.unwrap_or(Pos2::new(200.0, 200.0));
        let mut open = true;
        let current = job.preset.format;
        egui::Window::new("format-pick")
            .id(Id::new("format-pick-win"))
            .title_bar(false)
            .resizable(false)
            .collapsible(false)
            .fixed_pos(pos)
            .open(&mut open)
            .show(ui.ctx(), |ui| {
                for f in OutputFormat::all() {
                    if ui.selectable_label(*f == current, f.label()).clicked() {
                        self.handle.lock(|s| {
                            if let Some(j) = s.queue.jobs.iter_mut().find(|j| j.id == job_id) {
                                j.preset.format = *f;
                            }
                        });
                        self.format_popup_for = None;
                    }
                }
            });
        if !open {
            self.format_popup_for = None;
        }
    }

    fn save_job_as_user_preset(&mut self, job: &Job) {
        let name = rfd::FileDialog::new().set_file_name(&job.preset.name).save_file();
        let mut p = job.preset.clone();
        p.kind = PresetKind::User;
        p.id = format!("user.{}", sanitize(&p.name));
        p.group.clear();
        if let Some(_path) = name {
            let _ = self.handle.lock(|s| s.presets.upsert_user(p.clone()));
            self.handle.mark_dirty();
            self.status = format!("Saved user preset {}", p.name);
        } else if let Some(n) = prompt_fallback(&p.name) {
            p.name = n;
            p.id = format!("user.{}", sanitize(&p.name));
            let _ = self.handle.lock(|s| s.presets.upsert_user(p.clone()));
            self.handle.mark_dirty();
        }
    }

    pub fn preset_browser_ux(&mut self, ui: &mut Ui) {
        let t = self.t();
        ui.horizontal(|ui| {
            ui.label(RichText::new("Preset Browser").strong().size(14.0));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(RichText::new("drag a preset onto the queue").italics().size(11.0).color(t.text_faint));
            });
        });
        ui.add(egui::TextEdit::singleline(&mut self.preset_filter).hint_text("Search presets").desired_width(f32::INFINITY));
        let filter = self.preset_filter.to_ascii_lowercase();
        let nsel = self.selected_ids.len();
        if nsel > 0 {
            ui.label(RichText::new(format!("{nsel} output{} selected", if nsel == 1 { "" } else { "s" })).small().color(t.accent));
        }
        let presets = self.handle.lock(|s| s.presets.all());
        let browsing = self.browser_selected.clone();
        enum PresetRow<'a> {
            Header(&'static str),
            Item(&'a ExportPreset),
        }
        let mut rows: Vec<PresetRow<'_>> = Vec::new();
        rows.push(PresetRow::Header("Studio"));
        for p in presets.iter().filter(|p| PresetLibrary::is_studio_system(&p.id)) {
            if filter_preset(p, &filter) {
                rows.push(PresetRow::Item(p));
            }
        }
        rows.push(PresetRow::Header("User Presets & Groups"));
        let mut groups: Vec<String> = presets.iter().filter(|p| p.kind == PresetKind::User && !p.group.is_empty()).map(|p| p.group.clone()).collect();
        groups.sort();
        groups.dedup();
        for g in &groups {
            for p in presets.iter().filter(|p| p.kind == PresetKind::User && p.group == *g) {
                if filter_preset(p, &filter) {
                    rows.push(PresetRow::Item(p));
                }
            }
        }
        for p in presets.iter().filter(|p| p.kind == PresetKind::User && p.group.is_empty()) {
            if filter_preset(p, &filter) {
                rows.push(PresetRow::Item(p));
            }
        }
        rows.push(PresetRow::Header("System Presets"));
        for p in presets.iter().filter(|p| p.kind == PresetKind::System && !PresetLibrary::is_studio_system(&p.id)) {
            if filter_preset(p, &filter) {
                rows.push(PresetRow::Item(p));
            }
        }
        egui::ScrollArea::vertical().id_salt("preset-scroll").max_height(ui.available_height() - 160.0).show_rows(ui, 22.0, rows.len(), |ui, range| {
            for i in range {
                match rows[i] {
                    PresetRow::Header(h) => {
                        ui.label(RichText::new(h).small().color(t.text_faint).strong());
                    }
                    PresetRow::Item(p) => self.browser_item(ui, p, browsing.as_deref()),
                }
            }
        });
        ui.separator();
        self.details_card(ui, &presets);
    }

    fn browser_item(&mut self, ui: &mut Ui, p: &ExportPreset, browsing: Option<&str>) {
        let t = self.t();
        let selected = browsing == Some(p.id.as_str());
        let id = p.id.clone();
        let name = p.name.clone();
        let extra = if PresetLibrary::is_studio_system(&p.id) {
            p.bitrate_short()
        } else {
            p.format.short_label().to_string()
        };
        let inner = ui.dnd_drag_source(Id::new(("pdrag", &id)), id.clone(), |ui| {
            ui.horizontal(|ui| {
                preset_dot(ui, &id, &t);
                let lab = if PresetLibrary::is_studio_system(&p.id) {
                    format!("{name}")
                } else {
                    name.clone()
                };
                let r = ui.selectable_label(selected, lab);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(RichText::new(extra).small().color(t.text_dim));
                });
                r
            })
            .inner
        });
        let resp = inner.inner;
        if resp.clicked() {
            self.browser_selected = Some(id.clone());
        }
        if resp.double_clicked() {
            self.edit_preset = Some(p.clone());
            self.show_preset_edit = true;
        }
    }

    fn details_card(&mut self, ui: &mut Ui, presets: &[ExportPreset]) {
        let t = self.t();
        let id = self.browser_selected.clone().or_else(|| presets.first().map(|p| p.id.clone()));
        let Some(id) = id else {
            return;
        };
        let Some(p) = presets.iter().find(|p| p.id == id) else {
            return;
        };
        ui.horizontal(|ui| {
            ui.label(RichText::new(&p.name).strong());
            if PresetLibrary::is_studio_system(&p.id) {
                ui.label(RichText::new("Studio preset").small().color(t.accent));
            }
        });
        ui.label(RichText::new(format!("Format    {}", p.format.label())).small());
        ui.label(RichText::new(format!("Size      {}", if p.video.width.is_none() { "Matches each screen".into() } else { format!("{}×{}", p.video.width.unwrap_or(0), p.video.height.unwrap_or(0)) })).small());
        let fps = match p.video.frame_rate {
            FrameRate::Custom(f) => format!("{f} fps"),
            FrameRate::Source => "Match source".into(),
        };
        ui.label(RichText::new(format!("Frame rate  {fps}")).small());
        ui.label(RichText::new(format!("Bitrate     {}", p.bitrate_short())).small());
        ui.label(RichText::new(format!("In queue    {} outputs use it", self.handle.lock(|s| s.queue.jobs.iter().filter(|j| j.preset.id == p.id).count()))).small());
        ui.horizontal(|ui| {
            if ui.button("Edit…").clicked() {
                self.edit_preset = Some(p.clone());
                self.show_preset_edit = true;
            }
            let n = self.selected_ids.len().max(if self.handle.lock(|s| s.selected.is_some()) { 1 } else { 0 });
            let lab = if n == 0 { "Apply to selected".into() } else { format!("Apply to selected ({n})") };
            if ui.add_enabled(n > 0, egui::Button::new(lab).fill(t.accent)).clicked() {
                let mut ids = self.selected_ids.clone();
                if ids.is_empty() {
                    if let Some(sel) = self.handle.lock(|s| s.selected.clone()) {
                        ids.push(sel);
                    }
                }
                self.apply_preset_ids(&ids, &p.id);
            }
        });
        ui.label(RichText::new("Clicking a preset here does not change the default preset.").small().color(t.text_faint));
    }

    pub fn preset_edit_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show_preset_edit;
        let mut save = false;
        let mut save_as_user = false;
        let mut reset = false;
        let dim = self.t().text_dim;
        let t = self.t();
        if let Some(p) = self.edit_preset.as_mut() {
            let title = format!("Preset Settings — {}", p.name);
            egui::Window::new(title).open(&mut open).default_width(520.0).show(ctx, |ui| {
                if PresetLibrary::is_studio_system(&p.id) {
                    ui.label(RichText::new("Studio preset").color(t.accent).small());
                }
                ui.horizontal(|ui| {
                    ui.label("Name");
                    ui.text_edit_singleline(&mut p.name);
                    ui.label("Group");
                    ui.text_edit_singleline(&mut p.group);
                });
                ui.label(p.format.label());
                ui.separator();
                ui.label(RichText::new("VIDEO").small().strong());
                ui.horizontal(|ui| {
                    ui.label("Size");
                    let mut match_src = p.video.width.is_none() && p.video.height.is_none();
                    if ui.radio_value(&mut match_src, true, "Match each screen's size").clicked() {
                        p.video.width = None;
                        p.video.height = None;
                    }
                    if ui.radio_value(&mut match_src, false, "Fixed").clicked() && p.video.width.is_none() {
                        p.video.width = Some(1920);
                        p.video.height = Some(1080);
                    }
                    if !match_src {
                        let mut w = p.video.width.unwrap_or(1920) as i32;
                        let mut h = p.video.height.unwrap_or(1080) as i32;
                        ui.add(egui::DragValue::new(&mut w).range(16..=16384));
                        ui.label("×");
                        ui.add(egui::DragValue::new(&mut h).range(16..=16384));
                        p.video.width = Some(w as u32);
                        p.video.height = Some(h as u32);
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("Frame rate");
                    let mut fps_src = matches!(p.video.frame_rate, FrameRate::Source);
                    if ui.checkbox(&mut fps_src, "Match source").changed() {
                        p.video.frame_rate = if fps_src { FrameRate::Source } else { FrameRate::Custom(25.0) };
                    }
                    if let FrameRate::Custom(ref mut f) = p.video.frame_rate {
                        ui.add(egui::DragValue::new(f).range(1.0..=120.0).suffix(" fps"));
                    }
                });
                match &mut p.video.quality {
                    Quality::Vbr2Pass { target_mbps, max_mbps } => {
                        ui.label("Bitrate encoding    VBR, 2-pass");
                        ui.horizontal(|ui| {
                            ui.label("Target bitrate");
                            ui.add(egui::Slider::new(target_mbps, 0.5..=40.0).suffix(" Mbps"));
                        });
                        ui.horizontal(|ui| {
                            ui.label("Maximum bitrate");
                            ui.add(egui::Slider::new(max_mbps, 0.5..=50.0).suffix(" Mbps"));
                        });
                        let e10 = p.estimate_file_mb(10.0);
                        let e15 = p.estimate_file_mb(15.0);
                        ui.label(RichText::new(format!("Estimated file size   10 s ≈ {:.0} MB    15 s ≈ {:.0} MB", e10, e15)).small().color(dim));
                    }
                    Quality::Crf { value } => {
                        ui.add(egui::Slider::new(value, 0..=51).text("CRF"));
                    }
                    Quality::Bitrate { kbps } => {
                        ui.add(egui::DragValue::new(kbps).range(100..=200_000).prefix("kbps "));
                    }
                    Quality::Quality { value } => {
                        ui.add(egui::Slider::new(value, 1..=100).text("quality"));
                    }
                }
                ui.separator();
                ui.label(RichText::new("AUDIO").small().strong());
                ui.checkbox(&mut p.audio.enabled, "Include audio");
                if p.audio.enabled {
                    ui.add(egui::DragValue::new(&mut p.audio.bitrate_kbps).range(32..=512).prefix("kbps "));
                }
                ui.checkbox(&mut p.one_off, "One-off (don't remember as manual choice)");
                let n_using = self.handle.lock(|s| s.queue.jobs.iter().filter(|j| j.preset.id == p.id).count());
                if n_using > 0 {
                    ui.label(RichText::new(format!("{n_using} outputs in the queue use {}. Saving updates jobs that haven't started.", p.name)).small().color(dim));
                }
                ui.horizontal(|ui| {
                    if PresetLibrary::is_studio_system(&p.id) && ui.button("Reset to studio default").clicked() {
                        reset = true;
                    }
                    if ui.button("+ Save as user preset…").clicked() {
                        save_as_user = true;
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.add(egui::Button::new("Save").fill(t.accent)).clicked() {
                            save = true;
                        }
                    });
                });
            });
        }
        if reset {
            if let Some(id) = self.edit_preset.as_ref().map(|p| p.id.clone()) {
                if let Ok(p) = self.handle.lock(|s| s.presets.reset_studio_to_builtin(&id)) {
                    self.edit_preset = Some(p);
                }
            }
        }
        if save_as_user {
            if let Some(mut p) = self.edit_preset.clone() {
                p.kind = PresetKind::User;
                p.id = format!("user.{}", sanitize(&p.name));
                let _ = self.handle.lock(|s| s.presets.upsert_user(p));
                self.handle.mark_dirty();
                self.status = "Saved as user preset".into();
            }
        }
        if save {
            if let Some(p) = self.edit_preset.clone() {
                let pid = p.id.clone();
                match self.handle.lock(|s| s.presets.save_preset(p.clone()).or_else(|_| s.presets.upsert_user(p))) {
                    Ok(()) => {
                        self.status = "Preset saved".into();
                        self.handle.lock(|s| {
                            for j in &mut s.queue.jobs {
                                if j.preset.id == pid && !j.status.is_active() {
                                    let w = j.preset.video.width;
                                    let h = j.preset.video.height;
                                    if let Ok(next) = s.presets.get(&pid) {
                                        j.preset = next;
                                        j.preset.video.width = w;
                                        j.preset.video.height = h;
                                    }
                                }
                            }
                        });
                    }
                    Err(e) => self.status = e.to_string(),
                }
                self.handle.mark_dirty();
            }
            open = false;
        }
        self.show_preset_edit = open;
    }

    pub fn rules_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show_rules;
        let mut close = false;
        let t = self.t();
        egui::Window::new("Auto-assign rules — which screens get Better Res").open(&mut open).default_width(640.0).default_height(560.0).show(ctx, |ui| {
            let mut enabled = self.handle.lock(|s| s.presets.screen_rules.enabled);
            ui.checkbox(&mut enabled, "Pick Better Res or Installation automatically for every screen output");
            self.handle.lock(|s| s.presets.screen_rules.enabled = enabled);
            ui.separator();
            ui.label(RichText::new("1 · ULTRA WIDE  →  Better Res").strong());
            ui.horizontal(|ui| {
                ui.label("Landscape screens wider than");
                let mut aspect = self.handle.lock(|s| s.presets.screen_rules.ultra_wide_aspect);
                if ui.add(egui::DragValue::new(&mut aspect).range(1.0..=20.0).speed(0.1)).changed() {
                    self.handle.lock(|s| s.presets.screen_rules.ultra_wide_aspect = aspect);
                }
                ui.label(":1");
            });
            ui.label("Always count as ultra wide:");
            let mut names = self.handle.lock(|s| s.presets.screen_rules.ultra_wide_names.clone());
            ui.horizontal(|ui| {
                for (i, n) in names.clone().iter().enumerate() {
                    ui.label(RichText::new(n).small().color(t.accent));
                    if ui.small_button("×").clicked() {
                        names.remove(i);
                        self.handle.lock(|s| s.presets.screen_rules.ultra_wide_names = names.clone());
                    }
                }
            });
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut self.rules_new_ultra).desired_width(180.0).hint_text("Add screen"));
                if ui.button("+ Add screen").clicked() && !self.rules_new_ultra.trim().is_empty() {
                    self.handle.lock(|s| s.presets.screen_rules.ultra_wide_names.push(self.rules_new_ultra.trim().to_string()));
                    self.rules_new_ultra.clear();
                }
            });
            ui.separator();
            ui.label(RichText::new("2 · LISTED SCREENS  →  Better Res").strong());
            let listed = self.handle.lock(|s| s.presets.screen_rules.listed.clone());
            egui::Grid::new("listed-grid").num_columns(4).striped(true).show(ui, |ui| {
                ui.label(RichText::new("Screen").small().strong());
                ui.label(RichText::new("Size").small().strong());
                ui.label(RichText::new("Also matches names containing").small().strong());
                ui.end_row();
                let mut remove = None;
                for (i, r) in listed.iter().enumerate() {
                    ui.label(&r.name);
                    ui.label(match (r.width, r.height) {
                        (Some(w), Some(h)) => format!("{w}×{h}"),
                        _ => "—".into(),
                    });
                    ui.label(r.aliases.join(", "));
                    if ui.small_button("×").clicked() {
                        remove = Some(i);
                    }
                    ui.end_row();
                }
                if let Some(i) = remove {
                    self.handle.lock(|s| {
                        if i < s.presets.screen_rules.listed.len() {
                            s.presets.screen_rules.listed.remove(i);
                        }
                    });
                }
            });
            ui.horizontal(|ui| {
                if ui.button("+ Add from screen library…").clicked() {
                    self.pick_listed_from_library();
                }
            });
            ui.separator();
            ui.label(RichText::new("3 · EVERYTHING ELSE  →  Installation").strong());
            ui.horizontal(|ui| {
                ui.label("NO SCREEN NAME    a file dropped from Explorer");
                let mut def = self.handle.lock(|s| s.settings.default_preset_id.clone());
                let presets = self.handle.lock(|s| s.presets.all());
                let cur = presets.iter().find(|p| p.id == def).map(|p| p.name.clone()).unwrap_or(def.clone());
                egui::ComboBox::from_id_salt("default-preset-rules").selected_text(&cur).show_ui(ui, |ui| {
                    for p in &presets {
                        if ui.selectable_label(p.id == def, &p.name).clicked() {
                            def = p.id.clone();
                        }
                    }
                });
                self.handle.lock(|s| s.settings.default_preset_id = def);
                ui.label(RichText::new("your default preset").small().color(t.text_dim));
            });
            ui.separator();
            ui.horizontal(|ui| {
                ui.label("Try a screen:");
                ui.add(egui::TextEdit::singleline(&mut self.try_screen).desired_width(220.0).hint_text("Eye of Kuwait 7560×2100"));
                if ui.button("Try").clicked() || ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    self.try_result = Some(try_screen_copy(self.handle.lock(|s| s.presets.screen_rules.clone()), &self.try_screen));
                }
            });
            if let Some(r) = &self.try_result {
                ui.label(RichText::new(r).color(t.accent));
            }
            ui.horizontal(|ui| {
                if ui.button("Re-apply to Auto rows  ·  Manual rows stay").clicked() {
                    self.handle.lock(|s| s.queue.reapply_auto_rows(&s.presets.clone()));
                    self.handle.mark_dirty();
                    self.status = "Re-applied Auto rows".into();
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.add(egui::Button::new("Save rules").fill(t.accent)).clicked() {
                        self.handle.mark_dirty();
                        self.status = "Rules saved".into();
                        close = true;
                    }
                });
            });
        });
        self.show_rules = open && !close;
    }

    fn pick_listed_from_library(&mut self) {
        self.show_screen_picker = true;
    }

    pub fn screen_library_picker(&mut self, ctx: &egui::Context) {
        if !self.show_screen_picker {
            return;
        }
        let mut open = true;
        let screens = self.handle.lock(|s| s.presets.screens.clone());
        egui::Window::new("Add from screen library").open(&mut open).show(ctx, |ui| {
            for sz in &screens {
                if ui.button(sz.label()).clicked() {
                    self.handle.lock(|s| {
                        if !s.presets.screen_rules.listed.iter().any(|r| r.name == sz.name) {
                            s.presets.screen_rules.listed.push(ScreenMatchRule::new(&sz.name, &[], sz.width, sz.height, false));
                        }
                    });
                }
            }
        });
        self.show_screen_picker = open;
    }
}

fn group_has_locked(g: &SourceGroup, jobs: &[Job]) -> bool {
    g.job_ids.iter().any(|id| jobs.iter().any(|j| j.id == *id && j.status.is_order_locked()))
}

fn rest_ids(jobs: &[Job], moving: &[String]) -> Vec<String> {
    jobs.iter().filter(|j| !(moving.iter().any(|id| id == &j.id) && !j.status.is_order_locked())).map(|j| j.id.clone()).collect()
}

fn drop_dest_for_output(jobs: &[Job], hovered_id: &str, moving: &[String], top: bool) -> usize {
    let rest = rest_ids(jobs, moving);
    let idx = rest.iter().position(|id| id == hovered_id).unwrap_or(rest.len().saturating_sub(1));
    if top { idx } else { idx.saturating_add(1) }
}

fn drop_dest_for_source(jobs: &[Job], g: &SourceGroup, moving: &[String], top: bool) -> usize {
    let rest = rest_ids(jobs, moving);
    let members: Vec<&String> = g.job_ids.iter().filter(|id| rest.iter().any(|r| r == *id)).collect();
    if members.is_empty() {
        return rest.len();
    }
    if top {
        rest.iter().position(|id| id == members[0]).unwrap_or(0)
    } else {
        rest.iter().position(|id| id == members[members.len() - 1]).map(|i| i.saturating_add(1)).unwrap_or(rest.len())
    }
}

struct QueueGeom {
    rect: Rect,
    is_source: bool,
    source_key: String,
    job_id: Option<String>,
    locked: bool,
    job_ids: Vec<String>,
}

fn hit_geom(geoms: &[QueueGeom], pos: Pos2) -> Option<&QueueGeom> {
    geoms.iter().find(|g| g.rect.contains(pos)).or_else(|| {
        geoms
            .iter()
            .filter(|g| pos.x >= g.rect.left() && pos.x <= g.rect.right())
            .min_by(|a, b| {
                (a.rect.center().y - pos.y)
                    .abs()
                    .total_cmp(&(b.rect.center().y - pos.y).abs())
            })
    })
}

fn paint_grab_or_lock(ui: &mut Ui, locked: bool, t: &Tokens) {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(14.0, 16.0), Sense::hover());
    let painter = ui.painter();
    if locked {
        resp.on_hover_text("Already rendering · can't be moved");
        let c = rect.center();
        painter.rect_stroke(
            Rect::from_center_size(c + Vec2::new(0.0, 2.5), Vec2::new(8.0, 7.0)),
            CornerRadius::same(1),
            Stroke::new(1.3, t.text_faint),
            StrokeKind::Inside,
        );
        painter.circle_stroke(c + Vec2::new(0.0, -1.5), 2.8, Stroke::new(1.3, t.text_faint));
    } else {
        let x0 = rect.center().x - 5.0;
        let x1 = rect.center().x + 5.0;
        let y = rect.center().y;
        for dy in [-4.0_f32, 0.0, 4.0] {
            painter.line_segment([Pos2::new(x0, y + dy), Pos2::new(x1, y + dy)], Stroke::new(1.5, t.text_dim));
        }
    }
}

fn paint_insert_line(painter: &Painter, rect: Rect, top: bool, accent: Color32, dest: usize) {
    let y = if top { rect.top() } else { rect.bottom() };
    painter.hline(rect.x_range(), y, Stroke::new(3.0_f32, accent));
    painter.circle_filled(Pos2::new(rect.left() + 2.0, y), 4.0, accent);
    painter.circle_filled(Pos2::new(rect.right() - 2.0, y), 4.0, accent);
    let tip = format!("becomes #{n}", n = dest.saturating_add(1));
    let tip_pos = Pos2::new(rect.center().x, y + if top { -12.0 } else { 12.0 });
    let pill = Rect::from_center_size(tip_pos, Vec2::new(92.0, 18.0));
    painter.rect_filled(pill, CornerRadius::same(8), Color32::from_rgb(0x12, 0x1c, 0x2c));
    painter.rect_stroke(pill, CornerRadius::same(8), Stroke::new(1.0_f32, accent), StrokeKind::Inside);
    painter.text(tip_pos, Align2::CENTER_CENTER, tip, egui::FontId::proportional(11.0), accent);
}

fn paint_forbidden(painter: &Painter, rect: Rect, msg: &str) {
    let red = Color32::from_rgb(0xc4, 0x3c, 0x4a);
    painter.rect_stroke(rect, CornerRadius::ZERO, Stroke::new(1.8_f32, red), StrokeKind::Inside);
    painter.rect_filled(rect, CornerRadius::ZERO, Color32::from_rgba_unmultiplied(0x40, 0x10, 0x12, 90));
    let c = rect.left_center() + Vec2::new(18.0, 0.0);
    painter.circle_stroke(c, 7.0, Stroke::new(1.6, red));
    painter.line_segment([c + Vec2::new(-4.5, -4.5), c + Vec2::new(4.5, 4.5)], Stroke::new(1.6, red));
    painter.text(rect.center() + Vec2::new(10.0, 0.0), Align2::CENTER_CENTER, msg, egui::FontId::proportional(12.0), Color32::from_rgb(0xf0, 0xc0, 0xc0));
}

struct Cols {
    src: Rect,
    fmt: Rect,
    preset: Rect,
    out: Rect,
    status: Rect,
}

fn col_layout(rect: Rect) -> Cols {
    let w = rect.width();
    let status_w = 64.0;
    let fmt_w = 78.0;
    let preset_w = (w * 0.28).clamp(180.0, 300.0);
    let out_w = (w * 0.28).clamp(160.0, 360.0);
    let src_w = (w - fmt_w - preset_w - out_w - status_w).max(160.0);
    let mut x = rect.left();
    let src = Rect::from_min_size(Pos2::new(x, rect.top()), Vec2::new(src_w, rect.height()));
    x += src_w;
    let fmt = Rect::from_min_size(Pos2::new(x, rect.top()), Vec2::new(fmt_w, rect.height()));
    x += fmt_w;
    let preset = Rect::from_min_size(Pos2::new(x, rect.top()), Vec2::new(preset_w, rect.height()));
    x += preset_w;
    let out = Rect::from_min_size(Pos2::new(x, rect.top()), Vec2::new(out_w, rect.height()));
    x += out_w;
    let status = Rect::from_min_size(Pos2::new(x, rect.top()), Vec2::new(status_w, rect.height()));
    Cols { src, fmt, preset, out, status }
}

fn header_row(ui: &mut Ui, t: &Tokens) {
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(w, 20.0), Sense::hover());
    let cols = col_layout(rect);
    let painter = ui.painter();
    let col = t.text_faint;
    painter.text(cols.src.left_center() + Vec2::new(20.0, 0.0), egui::Align2::LEFT_CENTER, "Source / screen", egui::FontId::proportional(11.0), col);
    painter.text(cols.fmt.left_center(), egui::Align2::LEFT_CENTER, "Format", egui::FontId::proportional(11.0), col);
    painter.text(cols.preset.left_center(), egui::Align2::LEFT_CENTER, "Preset", egui::FontId::proportional(11.0), col);
    painter.text(cols.out.left_center(), egui::Align2::LEFT_CENTER, "Output file", egui::FontId::proportional(11.0), col);
    painter.text(cols.status.left_center(), egui::Align2::LEFT_CENTER, "Status", egui::FontId::proportional(11.0), col);
}

pub fn group_jobs(jobs: &[Job]) -> Vec<SourceGroup> {
    let mut out = Vec::new();
    for j in jobs {
        let key = j.source.group_key();
        if let Some(g) = out.iter_mut().find(|g: &&mut SourceGroup| g.key == key) {
            g.job_ids.push(j.id.clone());
        } else {
            let n_later = 1;
            out.push(SourceGroup {
                key,
                title: j.source.group_title(),
                subtitle: if j.source.is_effectcraft() {
                    format!("{n_later} from EffectCraft")
                } else {
                    "dropped from Explorer · no screen name".into()
                },
                job_ids: vec![j.id.clone()],
            });
        }
    }
    for g in &mut out {
        let n = g.job_ids.len();
        if g.subtitle.contains("EffectCraft") {
            g.subtitle = format!("{n} output{} from EffectCraft", if n == 1 { "" } else { "s" });
        }
    }
    out
}

fn studio_counts(jobs: &[Job]) -> (usize, usize) {
    let b = jobs.iter().filter(|j| j.preset.id == BETTER_RES_ID).count();
    let i = jobs.iter().filter(|j| j.preset.id == INSTALLATION_ID).count();
    (b, i)
}

fn studio_count_chip(ui: &mut Ui, color: Color32, text: &str) {
    egui::Frame::NONE.fill(color.gamma_multiply(0.25)).inner_margin(egui::Margin::symmetric(6, 2)).corner_radius(4.0).show(ui, |ui| {
        ui.label(RichText::new(text).size(11.0).color(color));
    });
}

fn preset_dot(ui: &mut Ui, id: &str, t: &Tokens) {
    let c = match id {
        BETTER_RES_ID => t.warning,
        INSTALLATION_ID => t.accent,
        "system.approval-res" => Color32::from_rgb(0x5c, 0xc8, 0x8a),
        "system.sultan" => Color32::from_rgb(0xb0, 0x7c, 0xe8),
        _ => t.text_dim,
    };
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
    ui.painter().circle_filled(rect.center(), 4.5, c);
}

fn assignment_tag(ui: &mut Ui, job: &Job, t: &Tokens) {
    let (label, fill) = if job.preset_overridden {
        ("Manual", Color32::from_rgb(0x3a, 0x32, 0x20))
    } else {
        match job.preset_reason {
            Some(PresetReason::UltraWide) => ("Auto · ultra wide", Color32::from_rgb(0x3a, 0x2e, 0x18)),
            Some(PresetReason::ListedScreen) => ("Auto · listed screen", Color32::from_rgb(0x1c, 0x32, 0x48)),
            Some(PresetReason::OtherScreen) | Some(PresetReason::StudioDefault) => ("Auto · other screen", Color32::from_rgb(0x2a, 0x2a, 0x2a)),
            Some(PresetReason::Default) => ("Default", Color32::from_rgb(0x2a, 0x2a, 0x2a)),
            Some(PresetReason::Manual) => ("Manual", Color32::from_rgb(0x3a, 0x32, 0x20)),
            None => return,
        }
    };
    egui::Frame::NONE.fill(fill).inner_margin(egui::Margin::symmetric(6, 2)).corner_radius(3.0).show(ui, |ui| {
        ui.label(RichText::new(label).size(10.5).color(t.tab_text_active));
    });
}

fn filter_preset(p: &ExportPreset, q: &str) -> bool {
    if q.is_empty() {
        return true;
    }
    let blob = format!("{} {} {} {}", p.name, p.id, p.format.label(), p.bitrate_short()).to_ascii_lowercase();
    blob.contains(q)
}

fn studio_pick_row(ui: &mut Ui, p: &ExportPreset, selected: bool, t: &Tokens) -> egui::Response {
    ui.horizontal(|ui| {
        preset_dot(ui, &p.id, t);
        let r = ui.selectable_label(selected, &p.name);
        ui.label(RichText::new(format!("H.264 · 25 fps · {}", p.bitrate_short())).small().color(t.text_dim));
        r
    })
    .inner
}

fn sanitize(name: &str) -> String {
    let s: String = name.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).collect();
    let t = s.trim_matches('-').to_string();
    if t.is_empty() {
        "preset".into()
    } else {
        t
    }
}

fn prompt_fallback(name: &str) -> Option<String> {
    Some(format!("{name} copy"))
}

fn try_screen_copy(rules: encodecraft_presets::ScreenPresetRules, text: &str) -> String {
    let parsed = parse_try_screen(text);
    let (name, w, h) = parsed;
    let screen = if w > 0 { Some(ScreenSize::new(&name, w, h)) } else { None };
    match rules.pick(screen.as_ref(), &name, if w > 0 { Some((w, h)) } else { None }) {
        Some(p) => {
            let label = if p.preset_id == BETTER_RES_ID { "Better Res" } else { "Installation" };
            format!("{label}    {}    {w}×{h}", p.reason.tag_label())
        }
        None => "Default preset (no screen identity)".into(),
    }
}

fn parse_try_screen(text: &str) -> (String, u32, u32) {
    let t = text.trim();
    if let Some((name, rest)) = t.rsplit_once(' ') {
        if let Some((w, h)) = rest.to_ascii_lowercase().split_once('x').or_else(|| rest.split_once('×')) {
            if let (Ok(w), Ok(h)) = (w.trim().parse::<u32>(), h.trim().parse::<u32>()) {
                return (name.trim().to_string(), w, h);
            }
        }
    }
    if let Some((w, h)) = encodecraft_job::ScreenSize::parse_dims(t) {
        return (t.to_string(), w, h);
    }
    (t.to_string(), 0, 0)
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RevealPlan {
    Select(PathBuf),
    OpenDir(PathBuf),
}

/// Select the file if it exists; otherwise create the parent folder and open it.
fn plan_reveal(path: &Path) -> Result<RevealPlan, String> {
    if path.exists() {
        if path.is_dir() {
            return Ok(RevealPlan::OpenDir(path.to_path_buf()));
        }
        return Ok(RevealPlan::Select(path.to_path_buf()));
    }
    let dir = path.parent().filter(|p| !p.as_os_str().is_empty()).ok_or_else(|| "No output folder".to_string())?;
    std::fs::create_dir_all(dir).map_err(|e| format!("Couldn't create output folder ({e})"))?;
    Ok(RevealPlan::OpenDir(dir.to_path_buf()))
}

fn spawn_reveal(plan: &RevealPlan) -> Result<(), String> {
    match plan {
        RevealPlan::Select(p) => spawn_select_file(p),
        RevealPlan::OpenDir(p) => spawn_open_dir(p),
    }
}

fn spawn_detached(mut cmd: Command) -> Result<(), String> {
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    cmd.spawn().map(|_| ()).map_err(|e| e.to_string())
}

#[cfg(windows)]
fn spawn_select_file(path: &Path) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    let mut cmd = Command::new("explorer.exe");
    cmd.raw_arg(format!(r#"/select,"{}""#, path.display()));
    spawn_detached(cmd)
}

#[cfg(windows)]
fn spawn_open_dir(path: &Path) -> Result<(), String> {
    let mut cmd = Command::new("explorer.exe");
    cmd.arg(path);
    spawn_detached(cmd)
}

#[cfg(target_os = "macos")]
fn spawn_select_file(path: &Path) -> Result<(), String> {
    spawn_detached({
        let mut cmd = Command::new("open");
        cmd.arg("-R").arg(path);
        cmd
    })
}

#[cfg(target_os = "macos")]
fn spawn_open_dir(path: &Path) -> Result<(), String> {
    spawn_detached({
        let mut cmd = Command::new("open");
        cmd.arg(path);
        cmd
    })
}

#[cfg(all(unix, not(target_os = "macos")))]
fn spawn_select_file(path: &Path) -> Result<(), String> {
    let dir = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(path);
    spawn_open_dir(dir)
}

#[cfg(all(unix, not(target_os = "macos")))]
fn spawn_open_dir(path: &Path) -> Result<(), String> {
    spawn_detached({
        let mut cmd = Command::new("xdg-open");
        cmd.arg(path);
        cmd
    })
}

#[cfg(test)]
mod reveal_tests {
    use super::*;
    use std::fs;

    fn tmp_root() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ec-reveal-{}-{}", std::process::id(), encodecraft_queue::unix_ms()));
        let _ = fs::create_dir_all(&dir);
        dir
    }

    #[test]
    fn selects_existing_file() {
        let dir = tmp_root();
        let file = dir.join("clip.mp4");
        let Ok(()) = fs::write(&file, b"x") else {
            return;
        };
        assert_eq!(plan_reveal(&file).ok(), Some(RevealPlan::Select(file)));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn opens_parent_when_file_not_rendered() {
        let dir = tmp_root();
        let file = dir.join("not-rendered.mp4");
        assert_eq!(plan_reveal(&file).ok(), Some(RevealPlan::OpenDir(dir.clone())));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn creates_missing_folder_then_opens_it() {
        let dir = tmp_root();
        let file = dir.join("no-such-folder").join("clip.mp4");
        let nested = dir.join("no-such-folder");
        assert_eq!(plan_reveal(&file).ok(), Some(RevealPlan::OpenDir(nested.clone())));
        assert!(nested.is_dir());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn opens_directory_path() {
        let dir = tmp_root();
        assert_eq!(plan_reveal(&dir).ok(), Some(RevealPlan::OpenDir(dir.clone())));
        let _ = fs::remove_dir_all(&dir);
    }
}
