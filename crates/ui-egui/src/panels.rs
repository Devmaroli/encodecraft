//! Named workspace surfaces. Today they are a fixed split; later they can be
//! `egui_dock` tabs without rewriting the panel bodies.

use egui::Ui;

use crate::{BottomTab, EncodeApp};

/// Surfaces that will become dockable tabs after the mockup is approved.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DockSurface {
    Queue,
    PresetBrowser,
    Inspector,
}

impl DockSurface {
    pub fn title(self) -> &'static str {
        match self {
            Self::Queue => "Queue",
            Self::PresetBrowser => "Preset Browser",
            Self::Inspector => "Inspector",
        }
    }
}

impl EncodeApp {
    /// Fixed workspace: queue | browser on top, inspector below.
    /// Swap this body for `egui_dock` when the pop-out mockup is approved.
    pub fn show_workspace(&mut self, ui: &mut Ui) {
        let t = self.t();
        let avail = ui.available_size();
        let bottom_h = (avail.y * 0.34).clamp(180.0, 320.0);
        let top_h = avail.y - bottom_h - 4.0;
        ui.allocate_ui(egui::Vec2::new(avail.x, top_h), |ui| {
            egui::SidePanel::right(DockSurface::PresetBrowser.title())
                .resizable(true)
                .default_width(280.0)
                .width_range(200.0..=420.0)
                .frame(egui::Frame::NONE.fill(t.panel_bg).inner_margin(8.0))
                .show_inside(ui, |ui| self.preset_browser_ux(ui));
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE.fill(t.app_bg).inner_margin(6.0))
                .show_inside(ui, |ui| self.queue_panel_ux(ui));
        });
        ui.separator();
        ui.allocate_ui(egui::Vec2::new(avail.x, bottom_h), |ui| {
            let _ = DockSurface::Inspector;
            self.bottom(ui);
        });
        let _ = BottomTab::Encode;
    }
}
