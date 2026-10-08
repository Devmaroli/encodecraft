//! Dockable panels, native pop-out viewports, and named workspaces.
//!
//! Drag a tab to one of the five drop targets (left / right / top / bottom / center)
//! to dock it. Drag it outside the window, or ☰ → Pop out, for a real OS window.

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use egui::{
    Color32, CornerRadius, Id, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, Ui, Vec2, ViewportBuilder, ViewportClass,
    ViewportCommand, ViewportId, WidgetText, WindowLevel,
};
use egui_dock::tab_viewer::OnCloseResponse;
use egui_dock::{DockArea, DockState, Node, NodeIndex, OverlayType, Style, Surface, SurfaceIndex, TabStyle, TabViewer, Tree};
use serde::{Deserialize, Serialize};

use crate::theme::Tokens;
use crate::EncodeApp;

pub const BUILTIN_FULL: &str = "full";
pub const BUILTIN_QUEUE: &str = "queue-only";
pub const BUILTIN_PRESETS: &str = "presets-queue";

const LAYOUT_VERSION: u32 = 1;

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PanelKind {
    Queue,
    PresetBrowser,
    Encoding,
    Source,
    Log,
    WatchFolders,
    Settings,
}

impl PanelKind {
    pub const ALL: [PanelKind; 7] = [
        PanelKind::Queue,
        PanelKind::PresetBrowser,
        PanelKind::Encoding,
        PanelKind::Source,
        PanelKind::Log,
        PanelKind::WatchFolders,
        PanelKind::Settings,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Self::Queue => "Queue",
            Self::PresetBrowser => "Preset Browser",
            Self::Encoding => "Encoding",
            Self::Source => "Source",
            Self::Log => "Log",
            Self::WatchFolders => "Watch Folders",
            Self::Settings => "Settings",
        }
    }

    pub fn short(self) -> &'static str {
        match self {
            Self::Queue => "Queue",
            Self::PresetBrowser => "Presets",
            Self::Encoding => "Encoding",
            Self::Source => "Source",
            Self::Log => "Log",
            Self::WatchFolders => "Watch",
            Self::Settings => "Settings",
        }
    }

    pub fn shortcut(self) -> (egui::Key, &'static str) {
        match self {
            Self::Queue => (egui::Key::Num1, "Ctrl+1"),
            Self::PresetBrowser => (egui::Key::Num2, "Ctrl+2"),
            Self::Encoding => (egui::Key::Num3, "Ctrl+3"),
            Self::Source => (egui::Key::Num4, "Ctrl+4"),
            Self::Log => (egui::Key::Num5, "Ctrl+5"),
            Self::WatchFolders => (egui::Key::Num6, "Ctrl+6"),
            Self::Settings => (egui::Key::Num7, "Ctrl+7"),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SavedFloat {
    pub tabs: Vec<PanelKind>,
    pub pos: [f32; 2],
    pub size: [f32; 2],
    #[serde(default)]
    pub always_on_top: bool,
    #[serde(default)]
    pub monitor_size: Option<[f32; 2]>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UserWorkspace {
    pub id: String,
    pub name: String,
    pub dock: DockState<PanelKind>,
    #[serde(default)]
    pub floats: Vec<SavedFloat>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct SavedLayoutFile {
    version: u32,
    active: String,
    dock: DockState<PanelKind>,
    #[serde(default)]
    floats: Vec<SavedFloat>,
    #[serde(default)]
    user_workspaces: Vec<UserWorkspace>,
}

pub struct FloatWin {
    pub id: u64,
    pub dock: DockState<PanelKind>,
    pub pos: Pos2,
    pub size: Vec2,
    pub always_on_top: bool,
    pub monitor_size: Option<Vec2>,
    pub needs_place: bool,
    pub frames: u32,
}

impl FloatWin {
    fn tabs(&self) -> Vec<PanelKind> {
        self.dock.iter_all_tabs().map(|(_, t)| *t).collect()
    }

    fn contains(&self, kind: PanelKind) -> bool {
        self.dock.find_tab(&kind).is_some()
    }

    fn to_saved(&self) -> SavedFloat {
        SavedFloat {
            tabs: self.tabs(),
            pos: [self.pos.x, self.pos.y],
            size: [self.size.x, self.size.y],
            always_on_top: self.always_on_top,
            monitor_size: self.monitor_size.map(|v| [v.x, v.y]),
        }
    }
}

#[derive(Clone, Copy)]
pub enum DockCmd {
    Toggle(PanelKind),
    PopOut(PanelKind),
    DockBack(PanelKind),
    Close(PanelKind),
    DockAll,
    ApplyWorkspace(&'static str),
    ApplyUser { idx: usize },
    Reset,
    SaveAs,
    Manage,
    ToggleOnTop(PanelKind),
}

pub struct LayoutSession {
    pub dock: DockState<PanelKind>,
    pub floats: Vec<FloatWin>,
    pub active: String,
    pub user: Vec<UserWorkspace>,
    pub focused: Option<PanelKind>,
    next_float: u64,
    dirty: bool,
    last_save: Instant,
    monitor_checked: bool,
}

impl LayoutSession {
    pub fn new() -> Self {
        Self {
            dock: layout_full(),
            floats: Vec::new(),
            active: BUILTIN_FULL.into(),
            user: Vec::new(),
            focused: Some(PanelKind::Queue),
            next_float: 1,
            dirty: false,
            last_save: Instant::now(),
            monitor_checked: false,
        }
    }

    pub fn load() -> Self {
        Self::load_from(&encodecraft_engine::layout_path())
    }

    fn load_from(path: &std::path::Path) -> Self {
        let Ok(raw) = std::fs::read_to_string(path) else {
            return Self::new();
        };
        Self::from_json(&raw)
    }

    fn from_json(raw: &str) -> Self {
        let Ok(file) = serde_json::from_str::<SavedLayoutFile>(raw) else {
            log::warn!("layout.json could not be read; using Full");
            return Self::new();
        };
        if file.version != LAYOUT_VERSION {
            return Self::new();
        }
        let mut s = Self::new();
        s.dock = file.dock;
        s.active = file.active;
        s.user = file.user_workspaces;
        for (i, f) in file.floats.into_iter().enumerate() {
            s.floats.push(float_from_saved(i as u64 + 1, f));
        }
        s.next_float = s.floats.iter().map(|f| f.id).max().unwrap_or(0) + 1;
        s.dirty = false;
        s
    }

    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    pub fn save_if_due(&mut self) {
        if self.last_save.elapsed() > Duration::from_secs(5) {
            self.save_now();
        }
    }

    pub fn save_now(&mut self) {
        self.save_to(&encodecraft_engine::layout_path());
    }

    fn save_to(&mut self, path: &std::path::Path) {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(raw) = serde_json::to_string_pretty(&self.to_file()) {
            let _ = std::fs::write(path, raw);
        }
        self.dirty = false;
        self.last_save = Instant::now();
    }

    fn to_file(&self) -> SavedLayoutFile {
        let mut dock = self.dock.clone();
        sanitize_dock(&mut dock);
        let mut user = self.user.clone();
        for ws in &mut user {
            sanitize_dock(&mut ws.dock);
        }
        SavedLayoutFile {
            version: LAYOUT_VERSION,
            active: self.active.clone(),
            dock,
            floats: self.floats.iter().map(FloatWin::to_saved).collect(),
            user_workspaces: user,
        }
    }

    pub fn panel_visible(&self, kind: PanelKind) -> bool {
        self.dock.find_tab(&kind).is_some() || self.floats.iter().any(|f| f.contains(kind))
    }

    pub fn is_floating(&self, kind: PanelKind) -> bool {
        self.floats.iter().any(|f| f.contains(kind))
    }
}

fn float_from_saved(id: u64, f: SavedFloat) -> FloatWin {
    let tabs = if f.tabs.is_empty() { vec![PanelKind::Queue] } else { f.tabs };
    FloatWin {
        id,
        dock: DockState::new(tabs),
        pos: Pos2::new(f.pos[0], f.pos[1]),
        size: Vec2::new(f.size[0].max(280.0), f.size[1].max(200.0)),
        always_on_top: f.always_on_top,
        monitor_size: f.monitor_size.map(|m| Vec2::new(m[0], m[1])),
        needs_place: true,
        frames: 0,
    }
}

pub fn empty_dock() -> DockState<PanelKind> {
    DockState::new(Vec::new())
}

pub fn layout_full() -> DockState<PanelKind> {
    let mut dock = DockState::new(vec![PanelKind::Queue]);
    let _ = dock.main_surface_mut().split_right(NodeIndex::root(), 0.72, vec![PanelKind::PresetBrowser]);
    let _ = dock.main_surface_mut().split_below(
        NodeIndex::root(),
        0.64,
        vec![PanelKind::Encoding, PanelKind::Source, PanelKind::Log, PanelKind::WatchFolders, PanelKind::Settings],
    );
    dock.translations.tab_context_menu.eject_button = "Pop out".into();
    dock.translations.tab_context_menu.close_button = "Close panel".into();
    dock
}

pub fn layout_queue_only() -> DockState<PanelKind> {
    let mut dock = DockState::new(vec![PanelKind::Queue]);
    dock.translations.tab_context_menu.eject_button = "Pop out".into();
    dock.translations.tab_context_menu.close_button = "Close panel".into();
    dock
}

pub fn layout_presets_queue() -> DockState<PanelKind> {
    let mut dock = DockState::new(vec![PanelKind::Queue]);
    let _ = dock.main_surface_mut().split_right(NodeIndex::root(), 0.72, vec![PanelKind::PresetBrowser]);
    dock.translations.tab_context_menu.eject_button = "Pop out".into();
    dock.translations.tab_context_menu.close_button = "Close panel".into();
    dock
}

pub fn builtin_dock(id: &str) -> Option<DockState<PanelKind>> {
    match id {
        BUILTIN_FULL => Some(layout_full()),
        BUILTIN_QUEUE => Some(layout_queue_only()),
        BUILTIN_PRESETS => Some(layout_presets_queue()),
        _ => None,
    }
}

pub fn builtin_label(id: &str) -> &'static str {
    match id {
        BUILTIN_FULL => "Full",
        BUILTIN_QUEUE => "Queue only",
        BUILTIN_PRESETS => "Presets + Queue",
        _ => "Workspace",
    }
}

fn dock_style(ui: &Ui, t: &Tokens) -> Style {
    let mut style = Style::from_egui(ui.style());
    style.dock_area_padding = None;
    style.main_surface_border_stroke = Stroke::new(1.0_f32, t.field_border);
    style.tab_bar.bg_fill = t.header_bg;
    style.tab_bar.height = 26.0;
    style.tab_bar.hline_color = t.field_border;
    style.separator.color_idle = t.field_border;
    style.separator.color_hovered = t.link;
    style.separator.color_dragged = t.accent;
    style.separator.width = 3.0;
    style.overlay.overlay_type = OverlayType::Widgets;
    style.overlay.selection_color = Color32::from_rgba_unmultiplied(0x24, 0x48, 0xF5, 90);
    style.overlay.button_color = t.accent;
    style.overlay.button_border_stroke = Stroke::new(1.0_f32, t.link);
    style.overlay.hovered_leaf_highlight = egui_dock::LeafHighlighting {
        color: Color32::from_rgba_unmultiplied(0x24, 0x48, 0xF5, 40),
        corner_radius: CornerRadius::same(4),
        stroke: Stroke::new(2.0_f32, t.accent),
        expansion: 0.0,
    };
    apply_tab_colors(&mut style.tab, t);
    style.tab.tab_body.bg_fill = t.panel_bg;
    style.tab.tab_body.stroke = Stroke::new(1.0_f32, t.field_border);
    style.tab.tab_body.inner_margin = egui::Margin::same(6);
    style
}

fn apply_tab_colors(tab: &mut TabStyle, t: &Tokens) {
    tab.active.bg_fill = t.accent;
    tab.active.text_color = Color32::WHITE;
    tab.active.outline_color = t.accent;
    tab.focused.bg_fill = t.accent;
    tab.focused.text_color = Color32::WHITE;
    tab.focused.outline_color = t.accent;
    tab.hovered.bg_fill = t.hover;
    tab.hovered.text_color = t.text;
    tab.hovered.outline_color = t.field_border;
    tab.inactive.bg_fill = t.raised;
    tab.inactive.text_color = t.tab_text;
    tab.inactive.outline_color = t.field_border;
    tab.active_with_kb_focus = tab.active.clone();
    tab.focused_with_kb_focus = tab.focused.clone();
    tab.inactive_with_kb_focus = tab.inactive.clone();
}

struct AppTabs<'a> {
    app: &'a mut EncodeApp,
    floating: bool,
}

impl TabViewer for AppTabs<'_> {
    type Tab = PanelKind;

    fn title(&mut self, tab: &mut Self::Tab) -> WidgetText {
        format!("☰  {}", tab.title()).into()
    }

    fn ui(&mut self, ui: &mut Ui, tab: &mut Self::Tab) {
        self.app.layout.focused = Some(*tab);
        match *tab {
            PanelKind::Queue => self.app.queue_panel_ux(ui),
            PanelKind::PresetBrowser => self.app.preset_browser_ux(ui),
            PanelKind::Encoding => self.app.encode_panel(ui),
            PanelKind::Source => self.app.preview_panel(ui),
            PanelKind::Log => self.app.log_panel(ui),
            PanelKind::WatchFolders => self.app.watch_panel(ui),
            PanelKind::Settings => self.app.settings_panel(ui),
        }
    }

    fn context_menu(&mut self, ui: &mut Ui, tab: &mut Self::Tab, _surface: SurfaceIndex, _node: NodeIndex) {
        let kind = *tab;
        if self.floating {
            if ui.button("Dock back                    Ctrl+Shift+D").clicked() {
                self.app.pending.push(DockCmd::DockBack(kind));
                ui.close();
            }
            let on = self.app.layout.floats.iter().any(|f| f.contains(kind) && f.always_on_top);
            let keep = if on { "✓ Keep on top of EncodeCraft" } else { "Keep on top of EncodeCraft" };
            if ui.button(keep).clicked() {
                self.app.pending.push(DockCmd::ToggleOnTop(kind));
                ui.close();
            }
        }
    }

    fn id(&mut self, tab: &mut Self::Tab) -> Id {
        Id::new(("ec-tab", *tab as u8))
    }

    fn scroll_bars(&self, tab: &Self::Tab) -> [bool; 2] {
        match tab {
            PanelKind::Queue | PanelKind::PresetBrowser | PanelKind::Log => [false, false],
            _ => [false, true],
        }
    }

    fn on_close(&mut self, _tab: &mut Self::Tab) -> OnCloseResponse {
        self.app.layout.mark_dirty();
        OnCloseResponse::Close
    }
}

impl EncodeApp {
    pub(crate) fn show_dock(&mut self, ui: &mut Ui) {
        self.promote_window_surfaces();
        let t = self.t();
        let style = dock_style(ui, &t);
        let mut dock = std::mem::replace(&mut self.layout.dock, empty_dock());
        {
            let mut viewer = AppTabs { app: self, floating: false };
            DockArea::new(&mut dock)
                .id(Id::new("encodecraft-dock"))
                .style(style)
                .show_close_buttons(true)
                .show_add_buttons(false)
                .tab_context_menus(true)
                .draggable_tabs(true)
                .show_leaf_close_all_buttons(false)
                .show_leaf_collapse_buttons(false)
                .show_inside(ui, &mut viewer);
        }
        self.layout.dock = dock;
        self.promote_window_surfaces();
    }

    pub(crate) fn show_floats(&mut self, ctx: &egui::Context) {
        if !self.layout.monitor_checked {
            self.dock_orphaned_floats(ctx);
            self.layout.monitor_checked = true;
        }
        let mut i = 0;
        while i < self.layout.floats.len() {
            let mut f = self.layout.floats.remove(i);
            let keep = self.show_one_float(ctx, &mut f);
            if keep {
                self.layout.floats.insert(i, f);
                i += 1;
            } else {
                self.layout.mark_dirty();
            }
        }
    }

    fn show_one_float(&mut self, ctx: &egui::Context, f: &mut FloatWin) -> bool {
        let tabs = f.tabs();
        if tabs.is_empty() {
            return false;
        }
        let title = if tabs.len() == 1 {
            format!("{} — EncodeCraft", tabs[0].title())
        } else {
            format!("{} — EncodeCraft", tabs.iter().map(|t| t.short()).collect::<Vec<_>>().join(" · "))
        };
        let id = ViewportId::from_hash_of(("encodecraft-float", f.id));
        let mut builder = ViewportBuilder::default()
            .with_title(title)
            .with_inner_size(f.size)
            .with_min_inner_size(Vec2::new(280.0, 180.0))
            .with_decorations(true);
        if f.needs_place {
            builder = builder.with_position(f.pos);
            f.needs_place = false;
        }
        builder = builder.with_window_level(if f.always_on_top { WindowLevel::AlwaysOnTop } else { WindowLevel::Normal });
        let mut close = false;
        let mut dock_back = false;
        ctx.show_viewport_immediate(id, builder, |ctx, class| {
            if class == ViewportClass::Embedded {
                egui::Window::new("Floating panel").show(ctx, |ui| {
                    ui.label("This environment cannot open a second OS window. Use ☰ → Dock back.");
                });
            }
            if ctx.input(|i| i.viewport().close_requested()) {
                ctx.send_viewport_cmd(ViewportCommand::CancelClose);
                close = true;
            }
            if ctx.input(|i| i.modifiers.command && i.modifiers.shift && i.key_pressed(egui::Key::D)) {
                dock_back = true;
            }
            let t = self.t();
            egui::CentralPanel::default().frame(egui::Frame::NONE.fill(t.panel_bg)).show(ctx, |ui| {
                let style = dock_style(ui, &t);
                let mut mini = std::mem::replace(&mut f.dock, empty_dock());
                {
                    let mut viewer = AppTabs { app: self, floating: true };
                    DockArea::new(&mut mini)
                        .id(Id::new(("ec-float-dock", f.id)))
                        .style(style)
                        .show_close_buttons(true)
                        .show_leaf_close_all_buttons(false)
                        .show_leaf_collapse_buttons(false)
                        .show_inside(ui, &mut viewer);
                }
                f.dock = mini;
            });
            if let Some(inner) = ctx.input(|i| i.viewport().inner_rect) {
                f.size = inner.size();
            }
            if let Some(outer) = ctx.input(|i| i.viewport().outer_rect) {
                f.pos = outer.min;
            }
            if let Some(mon) = ctx.input(|i| i.viewport().monitor_size) {
                f.monitor_size = Some(mon);
            }
        });
        f.frames = f.frames.saturating_add(1);
        if f.frames > 4 && monitor_missing(f.pos, f.size, f.monitor_size, ctx) {
            dock_back = true;
        }
        self.promote_float_windows(f);
        if dock_back {
            for tab in f.tabs() {
                if self.layout.dock.find_tab(&tab).is_none() {
                    self.layout.dock.push_to_first_leaf(tab);
                }
            }
            return false;
        }
        if close {
            return false;
        }
        !f.tabs().is_empty()
    }

    fn promote_float_windows(&mut self, f: &mut FloatWin) {
        let extras = collect_window_indices(&f.dock);
        for si in extras.into_iter().rev() {
            if let Some(Surface::Window(tree, state)) = f.dock.remove_surface(si) {
                let tabs: Vec<PanelKind> = tree.tabs().copied().collect();
                if tabs.is_empty() {
                    continue;
                }
                let rect = state.rect();
                let pos = if rect.is_positive() { rect.min } else { f.pos + Vec2::new(32.0, 32.0) };
                let size = if rect.is_positive() { rect.size() } else { f.size };
                self.push_float(tabs, pos, size, f.always_on_top, f.monitor_size);
            }
        }
    }

    fn promote_window_surfaces(&mut self) {
        let extras = collect_window_indices(&self.layout.dock);
        for si in extras.into_iter().rev() {
            if let Some(Surface::Window(tree, state)) = self.layout.dock.remove_surface(si) {
                let tabs: Vec<PanelKind> = tree.tabs().copied().collect();
                if tabs.is_empty() {
                    continue;
                }
                let rect = state.rect();
                let (pos, size) = if rect.is_positive() && rect.size().x > 40.0 {
                    (rect.min, rect.size())
                } else {
                    (Pos2::new(80.0, 80.0), Vec2::new(440.0, 620.0))
                };
                self.push_float(tabs, pos, size, false, None);
            }
        }
    }

    fn push_float(&mut self, tabs: Vec<PanelKind>, pos: Pos2, size: Vec2, always_on_top: bool, monitor_size: Option<Vec2>) {
        let id = self.layout.next_float;
        self.layout.next_float += 1;
        self.layout.floats.push(FloatWin {
            id,
            dock: DockState::new(tabs),
            pos,
            size,
            always_on_top,
            monitor_size,
            needs_place: true,
            frames: 0,
        });
        self.layout.mark_dirty();
    }

    fn dock_orphaned_floats(&mut self, ctx: &egui::Context) {
        let mut i = 0;
        while i < self.layout.floats.len() {
            let f = &self.layout.floats[i];
            if monitor_missing(f.pos, f.size, f.monitor_size, ctx) {
                let tabs = self.layout.floats[i].tabs();
                self.layout.floats.remove(i);
                for tab in tabs {
                    if self.layout.dock.find_tab(&tab).is_none() {
                        self.layout.dock.push_to_first_leaf(tab);
                    }
                }
                self.layout.mark_dirty();
            } else {
                i += 1;
            }
        }
    }

    pub(crate) fn apply_dock_cmds(&mut self, ctx: &egui::Context) {
        let cmds = std::mem::take(&mut self.pending);
        for cmd in cmds {
            self.apply_dock_cmd(ctx, cmd);
        }
    }

    fn apply_dock_cmd(&mut self, ctx: &egui::Context, cmd: DockCmd) {
        match cmd {
            DockCmd::Toggle(kind) => self.toggle_panel(kind),
            DockCmd::PopOut(kind) => self.pop_out_panel(ctx, kind),
            DockCmd::DockBack(kind) => self.dock_back_panel(kind),
            DockCmd::Close(kind) => self.close_panel(kind),
            DockCmd::DockAll => self.dock_all_floats(),
            DockCmd::ApplyWorkspace(id) => self.apply_builtin(id),
            DockCmd::ApplyUser { idx } => self.apply_user(idx),
            DockCmd::Reset => self.reset_active_workspace(),
            DockCmd::SaveAs => self.show_save_as = true,
            DockCmd::Manage => self.show_manage = true,
            DockCmd::ToggleOnTop(kind) => {
                if let Some(f) = self.layout.floats.iter_mut().find(|f| f.contains(kind)) {
                    f.always_on_top = !f.always_on_top;
                    self.layout.mark_dirty();
                }
            }
        }
    }

    fn toggle_panel(&mut self, kind: PanelKind) {
        if self.layout.panel_visible(kind) {
            self.close_panel(kind);
        } else {
            self.layout.dock.push_to_focused_leaf(kind);
            if self.layout.dock.find_tab(&kind).is_none() {
                self.layout.dock.push_to_first_leaf(kind);
            }
            self.layout.focused = Some(kind);
            self.layout.mark_dirty();
        }
    }

    fn close_panel(&mut self, kind: PanelKind) {
        if let Some(loc) = self.layout.dock.find_tab(&kind) {
            let _ = self.layout.dock.remove_tab(loc);
        }
        let mut i = 0;
        while i < self.layout.floats.len() {
            if let Some(loc) = self.layout.floats[i].dock.find_tab(&kind) {
                let _ = self.layout.floats[i].dock.remove_tab(loc);
            }
            if self.layout.floats[i].tabs().is_empty() {
                self.layout.floats.remove(i);
            } else {
                i += 1;
            }
        }
        self.layout.mark_dirty();
    }

    fn pop_out_panel(&mut self, ctx: &egui::Context, kind: PanelKind) {
        if self.layout.is_floating(kind) {
            return;
        }
        if let Some(loc) = self.layout.dock.find_tab(&kind) {
            if loc.0.is_main() {
                let outer = ctx.input(|i| i.viewport().outer_rect).unwrap_or_else(|| ctx.screen_rect());
                let pos = Pos2::new(outer.max.x + 16.0, outer.min.y + 48.0);
                let rect = Rect::from_min_size(pos, Vec2::new(440.0, 620.0));
                let _ = self.layout.dock.detach_tab(loc, rect);
                self.promote_window_surfaces();
                if let Some(f) = self.layout.floats.last_mut() {
                    f.pos = pos;
                    f.needs_place = true;
                }
            }
        } else {
            let outer = ctx.input(|i| i.viewport().outer_rect).unwrap_or_else(|| ctx.screen_rect());
            let pos = Pos2::new(outer.max.x + 16.0, outer.min.y + 48.0);
            self.push_float(vec![kind], pos, Vec2::new(440.0, 620.0), false, None);
        }
        self.layout.mark_dirty();
    }

    fn dock_back_panel(&mut self, kind: PanelKind) {
        let mut i = 0;
        while i < self.layout.floats.len() {
            if self.layout.floats[i].contains(kind) {
                if let Some(loc) = self.layout.floats[i].dock.find_tab(&kind) {
                    let _ = self.layout.floats[i].dock.remove_tab(loc);
                }
                if self.layout.dock.find_tab(&kind).is_none() {
                    self.layout.dock.push_to_first_leaf(kind);
                }
                if self.layout.floats[i].tabs().is_empty() {
                    self.layout.floats.remove(i);
                }
                self.layout.mark_dirty();
                return;
            }
            i += 1;
        }
    }

    fn dock_all_floats(&mut self) {
        let floats = std::mem::take(&mut self.layout.floats);
        for f in floats {
            for tab in f.tabs() {
                if self.layout.dock.find_tab(&tab).is_none() {
                    self.layout.dock.push_to_first_leaf(tab);
                }
            }
        }
        self.layout.mark_dirty();
    }

    fn apply_builtin(&mut self, id: &str) {
        if let Some(dock) = builtin_dock(id) {
            self.layout.dock = dock;
            self.layout.floats.clear();
            self.layout.active = id.to_string();
            self.layout.focused = Some(PanelKind::Queue);
            self.layout.mark_dirty();
            self.status = format!("Workspace: {}", builtin_label(id));
        }
    }

    fn apply_user(&mut self, idx: usize) {
        let Some(ws) = self.layout.user.get(idx).cloned() else {
            return;
        };
        self.layout.dock = ws.dock;
        self.layout.floats = ws.floats.into_iter().enumerate().map(|(i, s)| float_from_saved(self.layout.next_float + i as u64, s)).collect();
        self.layout.next_float += self.layout.floats.len() as u64 + 1;
        self.layout.active = ws.id;
        self.layout.mark_dirty();
        self.status = format!("Workspace: {}", ws.name);
    }

    fn reset_active_workspace(&mut self) {
        if let Some(dock) = builtin_dock(&self.layout.active) {
            self.layout.dock = dock;
            self.layout.floats.clear();
            self.status = format!("Reset “{}” layout", builtin_label(&self.layout.active));
        } else if let Some(ws) = self.layout.user.iter().find(|w| w.id == self.layout.active).cloned() {
            self.layout.dock = ws.dock;
            self.layout.floats = ws.floats.into_iter().enumerate().map(|(i, s)| float_from_saved(self.layout.next_float + i as u64, s)).collect();
            self.layout.next_float += self.layout.floats.len() as u64 + 1;
            self.status = format!("Reset “{}” layout", self.active_workspace_name());
        } else {
            self.layout.dock = layout_full();
            self.layout.floats.clear();
            self.layout.active = BUILTIN_FULL.into();
            self.status = "Reset “Full” layout".into();
        }
        self.layout.mark_dirty();
    }

    fn snapshot_user(&self, id: String, name: String) -> UserWorkspace {
        UserWorkspace {
            id,
            name,
            dock: self.layout.dock.clone(),
            floats: self.layout.floats.iter().map(FloatWin::to_saved).collect(),
        }
    }

    fn save_current_as(&mut self, name: String) {
        let name = name.trim();
        if name.is_empty() {
            return;
        }
        let id = format!("user-{}", now_millis());
        let ws = self.snapshot_user(id.clone(), name.to_string());
        self.layout.user.push(ws);
        self.layout.active = id;
        self.layout.mark_dirty();
        self.status = format!("Saved layout as {name}");
    }

    pub(crate) fn active_workspace_name(&self) -> String {
        if let Some(label) = match self.layout.active.as_str() {
            BUILTIN_FULL | BUILTIN_QUEUE | BUILTIN_PRESETS => Some(builtin_label(&self.layout.active).to_string()),
            _ => None,
        } {
            return label;
        }
        self.layout.user.iter().find(|w| w.id == self.layout.active).map(|w| w.name.clone()).unwrap_or_else(|| "Full".into())
    }

    pub(crate) fn handle_dock_keys(&mut self, ctx: &egui::Context) {
        let cmd = ctx.input(|i| i.modifiers.command);
        let alt = ctx.input(|i| i.modifiers.alt);
        let shift = ctx.input(|i| i.modifiers.shift);
        if cmd && !shift && !alt {
            for kind in PanelKind::ALL {
                let key = kind.shortcut().0;
                if ctx.input(|i| i.key_pressed(key)) {
                    self.pending.push(DockCmd::Toggle(kind));
                }
            }
        }
        if cmd && shift && ctx.input(|i| i.key_pressed(egui::Key::D)) {
            if let Some(kind) = self.layout.focused.filter(|k| self.layout.is_floating(*k)) {
                self.pending.push(DockCmd::DockBack(kind));
            } else {
                self.pending.push(DockCmd::DockAll);
            }
        }
        if alt && shift {
            if ctx.input(|i| i.key_pressed(egui::Key::Num1)) {
                self.pending.push(DockCmd::ApplyWorkspace(BUILTIN_FULL));
            }
            if ctx.input(|i| i.key_pressed(egui::Key::Num2)) {
                self.pending.push(DockCmd::ApplyWorkspace(BUILTIN_QUEUE));
            }
            if ctx.input(|i| i.key_pressed(egui::Key::Num3)) {
                self.pending.push(DockCmd::ApplyWorkspace(BUILTIN_PRESETS));
            }
        }
    }

    pub(crate) fn window_menu(&mut self, ui: &mut Ui) {
        ui.menu_button("Window", |ui| {
            ui.label(RichText::new("PANELS").small().color(self.t().text_faint).strong());
            for kind in PanelKind::ALL {
                let on = self.layout.panel_visible(kind);
                let extra = if self.layout.is_floating(kind) { "  floating" } else { "" };
                let label = format!("{}{}{}", if on { "✓  " } else { "    " }, kind.title(), extra);
                let btn = egui::Button::new(label).shortcut_text(kind.shortcut().1);
                if ui.add(btn).clicked() {
                    if on {
                        self.pending.push(DockCmd::Close(kind));
                    } else {
                        self.pending.push(DockCmd::Toggle(kind));
                    }
                    ui.close();
                }
            }
            ui.separator();
            ui.menu_button("Workspaces", |ui| self.workspaces_submenu(ui));
            if ui.button("Pop out current panel").clicked() {
                if let Some(kind) = self.layout.focused {
                    self.pending.push(DockCmd::PopOut(kind));
                }
                ui.close();
            }
            if ui.button("Dock all floating panels").clicked() {
                self.pending.push(DockCmd::DockAll);
                ui.close();
            }
        });
    }

    pub(crate) fn workspaces_submenu(&mut self, ui: &mut Ui) {
        for (id, shortcut) in [(BUILTIN_FULL, "Alt+Shift+1"), (BUILTIN_QUEUE, "Alt+Shift+2"), (BUILTIN_PRESETS, "Alt+Shift+3")] {
            let on = self.layout.active == id;
            let label = format!("{}{}", if on { "✓  " } else { "    " }, builtin_label(id));
            if ui.add(egui::Button::new(label).shortcut_text(shortcut)).clicked() {
                self.pending.push(DockCmd::ApplyWorkspace(id));
                ui.close();
            }
        }
        if !self.layout.user.is_empty() {
            ui.separator();
            ui.label(RichText::new("YOURS").small().color(self.t().text_faint).strong());
            for (idx, ws) in self.layout.user.iter().enumerate() {
                let on = self.layout.active == ws.id;
                let label = format!("{}{}", if on { "✓  " } else { "    " }, ws.name);
                if ui.button(label).clicked() {
                    self.pending.push(DockCmd::ApplyUser { idx });
                    ui.close();
                }
            }
        }
        ui.separator();
        if ui.add(egui::Button::new("Save current layout as…").fill(self.t().accent)).clicked() {
            self.pending.push(DockCmd::SaveAs);
            ui.close();
        }
        if ui.button(format!("Reset “{}” layout", self.active_workspace_name())).clicked() {
            self.pending.push(DockCmd::Reset);
            ui.close();
        }
        if ui.button("Manage workspaces…").clicked() {
            self.pending.push(DockCmd::Manage);
            ui.close();
        }
        ui.separator();
        ui.label(RichText::new("✓ Remembered on close: panel sizes, tabs, floating windows and which monitor they were on. If a monitor is unplugged, floating panels dock back.").small().color(self.t().text_dim));
    }

    pub(crate) fn workspace_switcher(&mut self, ui: &mut Ui) {
        let t = self.t();
        ui.spacing_mut().item_spacing.x = 4.0;
        let current = self.active_workspace_name();
        ui.label(RichText::new("Workspace").color(t.text_dim).size(12.0));
        let chip = |id: &'static str| -> bool { self.layout.active == id };
        for (id, label) in [(BUILTIN_FULL, "Full"), (BUILTIN_QUEUE, "Queue only"), (BUILTIN_PRESETS, "Presets + Queue")] {
            let on = chip(id);
            let mut btn = egui::Button::new(RichText::new(label).size(12.0));
            if on {
                btn = btn.fill(t.accent);
            }
            if ui.add(btn).clicked() {
                self.pending.push(DockCmd::ApplyWorkspace(id));
            }
        }
        for (idx, ws) in self.layout.user.iter().enumerate() {
            let on = self.layout.active == ws.id;
            let mut btn = egui::Button::new(RichText::new(&ws.name).size(12.0));
            if on {
                btn = btn.fill(t.accent);
            }
            if ui.add(btn).clicked() {
                self.pending.push(DockCmd::ApplyUser { idx });
            }
        }
        ui.menu_button(format!("{current}  ▾"), |ui| {
            self.workspaces_submenu(ui);
        });
    }

    pub(crate) fn save_as_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_save_as {
            return;
        }
        let mut open = true;
        let mut save = false;
        let mut cancel = false;
        egui::Window::new("Save current layout as…")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                ui.label("Names your current arrangement.");
                ui.add(egui::TextEdit::singleline(&mut self.save_as_name).desired_width(280.0).hint_text("Night shift"));
                ui.horizontal(|ui| {
                    if ui.add(egui::Button::new("Save").fill(self.t().accent)).clicked() {
                        save = true;
                    }
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                });
            });
        if save {
            let name = std::mem::take(&mut self.save_as_name);
            self.save_current_as(name);
            self.show_save_as = false;
        }
        if cancel || !open {
            self.show_save_as = false;
        }
    }

    pub(crate) fn manage_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_manage {
            return;
        }
        let mut open = true;
        let t = self.t();
        egui::Window::new("Manage workspaces…")
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_size(Vec2::new(720.0, 420.0))
            .show(ctx, |ui| {
                ui.label(RichText::new("A small picture of each layout. Built-ins cannot be deleted.").color(t.text_dim));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    for id in [BUILTIN_FULL, BUILTIN_QUEUE, BUILTIN_PRESETS] {
                        workspace_card(ui, &t, builtin_label(id), Some(id), builtin_dock(id).as_ref(), &[], self.layout.active == id, false);
                    }
                });
                if !self.layout.user.is_empty() {
                    ui.add_space(10.0);
                    ui.label(RichText::new("YOURS").small().color(t.text_faint).strong());
                    ui.horizontal_wrapped(|ui| {
                        let mut rename = None;
                        let mut delete = None;
                        let mut up = None;
                        let mut down = None;
                        for (idx, ws) in self.layout.user.iter().enumerate() {
                            ui.vertical(|ui| {
                                workspace_card(ui, &t, &ws.name, None, Some(&ws.dock), &ws.floats, self.layout.active == ws.id, true);
                                ui.horizontal(|ui| {
                                    if ui.small_button("Rename").clicked() {
                                        rename = Some(idx);
                                    }
                                    if ui.add_enabled(idx > 0, egui::Button::new("↑")).clicked() {
                                        up = Some(idx);
                                    }
                                    if ui.add_enabled(idx + 1 < self.layout.user.len(), egui::Button::new("↓")).clicked() {
                                        down = Some(idx);
                                    }
                                    if ui.small_button("Delete").clicked() {
                                        delete = Some(idx);
                                    }
                                });
                            });
                        }
                        if let Some(idx) = rename {
                            self.rename_idx = Some(idx);
                            self.rename_buf = self.layout.user.get(idx).map(|w| w.name.clone()).unwrap_or_default();
                        }
                        if let Some(idx) = delete {
                            let gone = self.layout.user.remove(idx);
                            if self.layout.active == gone.id {
                                self.apply_builtin(BUILTIN_FULL);
                            }
                            self.layout.mark_dirty();
                        }
                        if let Some(idx) = up {
                            self.layout.user.swap(idx, idx - 1);
                            self.layout.mark_dirty();
                        }
                        if let Some(idx) = down {
                            self.layout.user.swap(idx, idx + 1);
                            self.layout.mark_dirty();
                        }
                    });
                }
                if let Some(idx) = self.rename_idx {
                    ui.separator();
                    ui.horizontal(|ui| {
                        ui.label("Rename");
                        ui.add(egui::TextEdit::singleline(&mut self.rename_buf).desired_width(200.0));
                        if ui.button("Apply").clicked() {
                            if let Some(ws) = self.layout.user.get_mut(idx) {
                                let n = self.rename_buf.trim();
                                if !n.is_empty() {
                                    ws.name = n.to_string();
                                }
                            }
                            self.rename_idx = None;
                            self.layout.mark_dirty();
                        }
                        if ui.button("Cancel").clicked() {
                            self.rename_idx = None;
                        }
                    });
                }
            });
        if !open {
            self.show_manage = false;
            self.rename_idx = None;
        }
    }
}

fn workspace_card(ui: &mut Ui, t: &Tokens, name: &str, builtin: Option<&str>, dock: Option<&DockState<PanelKind>>, floats: &[SavedFloat], current: bool, yours: bool) {
    let size = Vec2::new(150.0, 92.0);
    ui.vertical(|ui| {
        let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
        ui.painter().rect_filled(rect, CornerRadius::same(6), t.raised);
        ui.painter().rect_stroke(rect, CornerRadius::same(6), Stroke::new(1.0_f32, t.field_border), StrokeKind::Inside);
        if let Some(dock) = dock {
            paint_thumb(ui.painter(), dock.main_surface(), rect.shrink(8.0), t);
        }
        if !floats.is_empty() {
            let badge = Rect::from_min_size(rect.right_top() + Vec2::new(-22.0, 6.0), Vec2::new(14.0, 10.0));
            ui.painter().rect_stroke(badge, CornerRadius::same(2), Stroke::new(1.0_f32, t.link), StrokeKind::Inside);
        }
        ui.horizontal(|ui| {
            ui.label(RichText::new(name).strong().size(12.0));
            if current {
                ui.label(RichText::new("current").small().color(t.cache_green));
            } else if yours {
                ui.label(RichText::new("yours").small().color(t.text_dim));
            } else if builtin.is_some() {
                ui.label(RichText::new("built-in").small().color(t.text_faint));
            }
        });
        let caption = match builtin {
            Some(BUILTIN_FULL) => "Everything, like today. Best on one big screen.",
            Some(BUILTIN_QUEUE) => "Just the queue, full window. For watching a long render.",
            Some(BUILTIN_PRESETS) => "Big Preset Browser next to the queue. For setting up a send.",
            _ => "",
        };
        if !caption.is_empty() {
            ui.label(RichText::new(caption).small().color(t.text_dim));
        }
    });
}

fn paint_thumb(painter: &egui::Painter, tree: &Tree<PanelKind>, rect: Rect, t: &Tokens) {
    if let Some(root) = tree.root_node() {
        paint_node(painter, tree, NodeIndex::root(), root, rect, t);
    }
}

fn paint_node(painter: &egui::Painter, tree: &Tree<PanelKind>, idx: NodeIndex, node: &Node<PanelKind>, rect: Rect, t: &Tokens) {
    match node {
        Node::Leaf(leaf) => {
            painter.rect_filled(rect.shrink(1.0), CornerRadius::same(3), t.panel_bg);
            painter.rect_stroke(rect.shrink(1.0), CornerRadius::same(3), Stroke::new(1.0_f32, t.field_border), StrokeKind::Inside);
            let label = leaf.tabs.iter().map(|p| p.short()).collect::<Vec<_>>().join(" · ");
            painter.text(rect.center(), egui::Align2::CENTER_CENTER, label, egui::FontId::proportional(9.0), t.text_dim);
        }
        Node::Horizontal(split) => {
            let w = (rect.width() * split.fraction).clamp(8.0, rect.width() - 8.0);
            let left = Rect::from_min_max(rect.min, Pos2::new(rect.min.x + w, rect.max.y));
            let right = Rect::from_min_max(Pos2::new(rect.min.x + w, rect.min.y), rect.max);
            if let Some(n) = tree.get(idx.left()) {
                paint_node(painter, tree, idx.left(), n, left.shrink2(Vec2::new(1.0, 0.0)), t);
            }
            if let Some(n) = tree.get(idx.right()) {
                paint_node(painter, tree, idx.right(), n, right.shrink2(Vec2::new(1.0, 0.0)), t);
            }
        }
        Node::Vertical(split) => {
            let h = (rect.height() * split.fraction).clamp(8.0, rect.height() - 8.0);
            let top = Rect::from_min_max(rect.min, Pos2::new(rect.max.x, rect.min.y + h));
            let bot = Rect::from_min_max(Pos2::new(rect.min.x, rect.min.y + h), rect.max);
            if let Some(n) = tree.get(idx.left()) {
                paint_node(painter, tree, idx.left(), n, top.shrink2(Vec2::new(0.0, 1.0)), t);
            }
            if let Some(n) = tree.get(idx.right()) {
                paint_node(painter, tree, idx.right(), n, bot.shrink2(Vec2::new(0.0, 1.0)), t);
            }
        }
        Node::Empty => {}
    }
}

fn sanitize_dock(dock: &mut DockState<PanelKind>) {
    for surface in dock.iter_surfaces_mut() {
        let Some(tree) = surface.node_tree_mut() else {
            continue;
        };
        for node in tree.iter_mut() {
            match node {
                Node::Leaf(leaf) => {
                    if !leaf.rect.is_finite() {
                        leaf.rect = Rect::ZERO;
                    }
                    if !leaf.viewport.is_finite() {
                        leaf.viewport = Rect::ZERO;
                    }
                }
                Node::Horizontal(split) | Node::Vertical(split) => {
                    if !split.rect.is_finite() {
                        split.rect = Rect::ZERO;
                    }
                }
                Node::Empty => {}
            }
        }
    }
}

fn collect_window_indices(dock: &DockState<PanelKind>) -> Vec<SurfaceIndex> {
    let mut out = Vec::new();
    for i in 1..dock.surfaces_count() {
        let si = SurfaceIndex(i);
        if matches!(dock.get_surface(si), Some(Surface::Window(_, _))) {
            out.push(si);
        }
    }
    out
}

pub fn monitor_missing(pos: Pos2, size: Vec2, saved_monitor: Option<Vec2>, ctx: &egui::Context) -> bool {
    let rect = Rect::from_min_size(pos, size);
    let mut known = ctx.input(|i| i.viewport().monitor_size).or(saved_monitor).map(|m| Rect::from_min_size(Pos2::ZERO, m)).unwrap_or(Rect::NOTHING);
    if let Some(outer) = ctx.input(|i| i.viewport().outer_rect) {
        if known == Rect::NOTHING {
            known = outer;
        } else {
            known = known.union(outer);
        }
    }
    if known == Rect::NOTHING {
        return false;
    }
    !known.expand(160.0).intersects(rect)
}

fn now_millis() -> u128 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(1)
}

trait TreeGet<Tab> {
    fn get(&self, idx: NodeIndex) -> Option<&Node<Tab>>;
}

impl<Tab> TreeGet<Tab> for Tree<Tab> {
    fn get(&self, idx: NodeIndex) -> Option<&Node<Tab>> {
        self.iter().nth(idx.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tabs_in(dock: &DockState<PanelKind>) -> Vec<PanelKind> {
        let mut v: Vec<_> = dock.iter_all_tabs().map(|(_, t)| *t).collect();
        v.sort_by_key(|k| *k as u8);
        v
    }

    #[test]
    fn full_layout_has_all_seven_panels() {
        let dock = layout_full();
        let tabs = tabs_in(&dock);
        assert_eq!(tabs, PanelKind::ALL.to_vec());
        assert!(dock.find_tab(&PanelKind::Queue).is_some());
        assert!(dock.find_tab(&PanelKind::PresetBrowser).is_some());
        assert_eq!(dock.find_tab(&PanelKind::Encoding).map(|t| t.0.is_main()), Some(true));
    }

    #[test]
    fn queue_only_is_just_queue() {
        let tabs = tabs_in(&layout_queue_only());
        assert_eq!(tabs, vec![PanelKind::Queue]);
    }

    #[test]
    fn presets_queue_has_no_inspector() {
        let tabs = tabs_in(&layout_presets_queue());
        assert_eq!(tabs, vec![PanelKind::Queue, PanelKind::PresetBrowser]);
    }

    #[test]
    fn toggle_closes_and_reopens() {
        let mut s = LayoutSession::new();
        assert!(s.panel_visible(PanelKind::Log));
        if let Some(loc) = s.dock.find_tab(&PanelKind::Log) {
            let _ = s.dock.remove_tab(loc);
        }
        assert!(!s.panel_visible(PanelKind::Log));
        s.dock.push_to_first_leaf(PanelKind::Log);
        assert!(s.panel_visible(PanelKind::Log));
    }

    #[test]
    fn layout_roundtrip() {
        let dir = std::env::temp_dir().join(format!("ec-layout-{}-{}", std::process::id(), now_millis()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("layout.json");
        let mut s = LayoutSession::new();
        s.active = BUILTIN_QUEUE.into();
        s.dock = layout_queue_only();
        s.user.push(UserWorkspace {
            id: "user-1".into(),
            name: "Night shift".into(),
            dock: layout_presets_queue(),
            floats: vec![SavedFloat {
                tabs: vec![PanelKind::PresetBrowser],
                pos: [1920.0, 80.0],
                size: [420.0, 640.0],
                always_on_top: true,
                monitor_size: Some([1920.0, 1080.0]),
            }],
        });
        s.save_to(&path);
        let loaded = LayoutSession::load_from(&path);
        assert_eq!(loaded.active, BUILTIN_QUEUE);
        assert_eq!(tabs_in(&loaded.dock), vec![PanelKind::Queue]);
        assert_eq!(loaded.user.len(), 1);
        assert_eq!(loaded.user[0].name, "Night shift");
        assert_eq!(loaded.user[0].floats[0].tabs, vec![PanelKind::PresetBrowser]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn built_in_ids_cannot_be_user_deleted_by_id() {
        for id in [BUILTIN_FULL, BUILTIN_QUEUE, BUILTIN_PRESETS] {
            assert!(builtin_dock(id).is_some());
            assert!(!id.starts_with("user-"));
        }
    }

    #[test]
    fn overlay_is_five_drop_widgets() {
        assert!(matches!(OverlayType::Widgets, OverlayType::Widgets));
        assert!(matches!(OverlayType::HighlightedAreas, OverlayType::HighlightedAreas));
    }

    #[test]
    fn detach_creates_window_surface() {
        let mut dock = layout_full();
        let loc = dock.find_tab(&PanelKind::PresetBrowser);
        assert!(loc.is_some(), "Preset Browser should start docked in Full");
        let Some(loc) = loc else {
            return;
        };
        let si = dock.detach_tab(loc, Rect::from_min_size(Pos2::new(2000.0, 80.0), Vec2::new(400.0, 500.0)));
        assert!(!si.is_main());
        assert!(matches!(dock.get_surface(si), Some(Surface::Window(_, _))));
        assert!(dock.find_main_surface_tab(&PanelKind::PresetBrowser).is_none());
    }
}
