//! Workspace shell. Panel bodies stay on `EncodeApp`; docking lives in `dock`.

use egui::Ui;

use crate::EncodeApp;

impl EncodeApp {
    pub fn show_workspace(&mut self, ui: &mut Ui) {
        self.show_dock(ui);
    }
}
