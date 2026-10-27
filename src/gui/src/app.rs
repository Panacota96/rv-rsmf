//! The window. Drawing only: rules live in `state.rs`, work lives in the core (on a worker thread).
//! Rakvor dark theme; interface language EN/ES/FR switchable at any time (texts from i18n "ui").

use crate::state::{COMMON_ZONES, GuiState, Phase};
use eframe::egui;
use rv_rsmf_core::{
    Event, HashAlgo, RunConfig, RunSummary, TOOL_VERSION,
    model::SourceKind,
    sources::{CsvProfile, DateOrder},
};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

// Rakvor palette
const NIGHT: egui::Color32 = egui::Color32::from_rgb(0x0D, 0x11, 0x17);
const NIGHT2: egui::Color32 = egui::Color32::from_rgb(0x1A, 0x1F, 0x26);
const BLUE: egui::Color32 = egui::Color32::from_rgb(0x2A, 0x5C, 0xFF);
const VIOLET: egui::Color32 = egui::Color32::from_rgb(0x6B, 0x00, 0xFF);
const CYAN: egui::Color32 = egui::Color32::from_rgb(0x00, 0xE5, 0xFF);
const MIST: egui::Color32 = egui::Color32::from_rgb(0xE6, 0xE6, 0xE6);
const AMBER: egui::Color32 = egui::Color32::from_rgb(0xFF, 0xB0, 0x40);
const RED: egui::Color32 = egui::Color32::from_rgb(0xFF, 0x6B, 0x6B);
const GREEN: egui::Color32 = egui::Color32::from_rgb(0x4C, 0xD9, 0x7B);
const UI_LANGS: [(&str, &str); 3] = [("en", "English"), ("es", "Español"), ("fr", "Français")];

#[allow(clippy::large_enum_variant)] // events are small and short-lived; boxing adds nothing measurable
enum Msg {
    Event(Event),
    Done(Result<RunSummary, String>),
}

pub struct HashInvApp {
    state: GuiState,
    rx: Option<Receiver<Msg>>,
    cancel: Arc<AtomicBool>,
    started: Option<Instant>,
    notice: Option<String>,
    themed: bool,
    /// Case folder being typed; applied (and audited) when the field loses focus.
    case_entry: String,
    /// Chat-export text fields being typed; applied (and audited) when the field loses focus.
    tz_entry: String,
    custodian_entry: String,
    csv_map_entry: String,
    platform_entry: String,
}

/// Applies the Rakvor dark theme.
pub fn apply_theme(ctx: &egui::Context) {
    let mut v = egui::Visuals::dark();
    v.panel_fill = NIGHT;
    v.window_fill = NIGHT2;
    v.extreme_bg_color = NIGHT2; // text fields
    v.widgets.inactive.bg_stroke =
        egui::Stroke::new(1.0, egui::Color32::from_rgb(0x2B, 0x32, 0x40));
    v.widgets.noninteractive.bg_stroke =
        egui::Stroke::new(1.0, egui::Color32::from_rgb(0x2B, 0x32, 0x40));
    v.faint_bg_color = NIGHT2;
    v.override_text_color = Some(MIST);
    v.selection.bg_fill = BLUE;
    v.hyperlink_color = CYAN;
    v.widgets.inactive.bg_fill = NIGHT2;
    v.widgets.inactive.weak_bg_fill = NIGHT2;
    v.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, CYAN);
    v.widgets.active.bg_fill = BLUE;
    ctx.set_visuals(v);
}

impl HashInvApp {
    pub fn new() -> Self {
        HashInvApp {
            state: GuiState::default(),
            rx: None,
            cancel: Arc::new(AtomicBool::new(false)),
            started: None,
            notice: None,
            themed: false,
            case_entry: String::new(),
            tz_entry: String::new(),
            custodian_entry: String::new(),
            csv_map_entry: String::new(),
            platform_entry: String::new(),
        }
    }

    /// Starts with evidence already selected (paths given to the executable).
    pub fn with_inputs(paths: impl IntoIterator<Item = PathBuf>) -> Self {
        let mut app = Self::new();
        for p in paths {
            app.state.add_input(p);
        }
        app
    }

    fn start(&mut self, ctx: &egui::Context) {
        // Rule (state.rs): exports are previewed with the current settings; a usage problem stops here.
        if let Err(e) = self.state.check_exports() {
            self.notice = Some(e);
            return;
        }
        let cfg: RunConfig = match self.state.to_config() {
            Ok(c) => c,
            Err(e) => {
                self.notice = Some(e);
                return;
            }
        };
        self.notice = None;
        self.state.start();
        self.cancel.store(false, Ordering::SeqCst);
        self.started = Some(Instant::now());
        let (tx, rx) = channel();
        self.rx = Some(rx);
        let cancel = self.cancel.clone();
        let repaint = ctx.clone();
        std::thread::spawn(move || {
            let tx2 = tx.clone();
            let r2 = repaint.clone();
            let mut sink = move |e: &Event| {
                let _ = tx2.send(Msg::Event(e.clone()));
                r2.request_repaint();
            };
            let result = rv_rsmf_core::run(&cfg, &mut sink, &cancel);
            let _ = tx.send(Msg::Done(result));
            repaint.request_repaint();
        });
    }

    fn drain(&mut self) {
        let Some(rx) = &self.rx else { return };
        let mut finished = false;
        while let Ok(m) = rx.try_recv() {
            match m {
                Msg::Event(e) => {
                    if let Event::Timestamp { status, reason, .. } = &e
                        && status != "ok"
                    {
                        self.state
                            .warnings
                            .push(format!("timestamp: {}", reason.as_deref().unwrap_or("-")));
                    }
                    self.state.apply(&e)
                }
                Msg::Done(Ok(summary)) => {
                    self.state.finish(&summary);
                    finished = true;
                }
                Msg::Done(Err(e)) => {
                    self.state.fail(e);
                    finished = true;
                }
            }
        }
        if finished {
            self.rx = None;
            self.started = None;
        }
    }

    fn header(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(38.0, 38.0), egui::Sense::hover());
            ui.painter().rect(
                rect,
                8.0,
                NIGHT2,
                egui::Stroke::new(2.0, BLUE),
                egui::StrokeKind::Inside,
            );
            ui.painter().text(
                rect.center() - egui::vec2(0.0, 3.0),
                egui::Align2::CENTER_CENTER,
                "RV",
                egui::FontId::proportional(16.0),
                MIST,
            );
            ui.painter().line_segment(
                [
                    rect.center() + egui::vec2(-8.0, 10.0),
                    rect.center() + egui::vec2(8.0, 10.0),
                ],
                egui::Stroke::new(2.0, CYAN),
            );
            ui.vertical(|ui| {
                ui.label(egui::RichText::new("R A K V O R").strong().size(16.0));
                let desc = rv_rsmf_core::i18n::catalog(&self.state.ui_lang).brand()["descriptor"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                ui.label(egui::RichText::new(desc).color(CYAN).size(10.0));
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let current = UI_LANGS
                    .iter()
                    .find(|(k, _)| *k == self.state.ui_lang)
                    .map(|(_, v)| *v)
                    .unwrap_or("English");
                let mut chosen = None;
                egui::ComboBox::from_id_salt("ui-lang")
                    .selected_text(current)
                    .show_ui(ui, |ui| {
                        for (k, v) in UI_LANGS {
                            if ui.selectable_label(self.state.ui_lang == k, v).clicked() {
                                chosen = Some(k);
                            }
                        }
                    });
                if let Some(k) = chosen {
                    self.state.set_ui_lang(k);
                }
                ui.label(self.state.tr("ui_lang"));
                ui.weak(format!("v{}", TOOL_VERSION));
            });
        });
        ui.weak(self.state.tr("audit_note"));
        let (r, _) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 3.0), egui::Sense::hover());
        let third = r.width() / 3.0;
        for (i, c) in [BLUE, VIOLET, CYAN].iter().enumerate() {
            ui.painter().rect_filled(
                egui::Rect::from_min_size(
                    r.min + egui::vec2(third * i as f32, 0.0),
                    egui::vec2(third, 3.0),
                ),
                0.0,
                *c,
            );
        }
    }

    fn section_title(ui: &mut egui::Ui, text: String) {
        ui.add_space(4.0);
        ui.label(egui::RichText::new(text).color(CYAN).strong().size(15.0));
    }

    fn evidence_section(&mut self, ui: &mut egui::Ui, enabled: bool) {
        Self::section_title(ui, self.state.tr("evidence"));
        ui.label(self.state.tr("evidence_help"));
        ui.add_enabled_ui(enabled, |ui| {
            ui.horizontal(|ui| {
                #[cfg(windows)]
                {
                    if ui.button(self.state.tr("add_files")).clicked() {
                        if let Some(files) = rfd::FileDialog::new().pick_files() {
                            for f in files {
                                self.state.add_input(f);
                            }
                        }
                    }
                    if ui.button(self.state.tr("add_folder")).clicked() {
                        if let Some(d) = rfd::FileDialog::new().pick_folder() {
                            self.state.add_input(d);
                        }
                    }
                }
                let hint = self.state.tr("path_hint");
                let entry = ui.add(
                    egui::TextEdit::singleline(&mut self.state.path_entry)
                        .hint_text(hint)
                        .desired_width(320.0),
                );
                let enter = entry.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if ui.button(self.state.tr("add_path")).clicked() || enter {
                    self.state.add_typed_path();
                }
            });
            let dropped: Vec<PathBuf> = ui.ctx().input(|i| {
                i.raw
                    .dropped_files
                    .iter()
                    .map(|f| f.path().to_path_buf())
                    .filter(|p| !p.as_os_str().is_empty())
                    .collect()
            });
            for p in dropped {
                self.state.add_input(p);
            }
            let mut remove = None;
            egui::ScrollArea::vertical()
                .max_height(100.0)
                .id_salt("inputs")
                .show(ui, |ui| {
                    if self.state.inputs.is_empty() {
                        ui.weak(self.state.tr("no_evidence"));
                    }
                    for (i, p) in self.state.inputs.iter().enumerate() {
                        ui.horizontal(|ui| {
                            if ui.small_button("x").clicked() {
                                remove = Some(i);
                            }
                            ui.monospace(p.display().to_string());
                        });
                    }
                });
            if let Some(i) = remove {
                self.state.remove_input(i);
            }
            let mut rec = self.state.recursive;
            if ui.checkbox(&mut rec, self.state.tr("subfolders")).changed() {
                self.state.set_recursive(rec);
            }
            ui.horizontal(|ui| {
                ui.label(self.state.tr("case_dir"));
                let hint = self.state.tr("case_dir_hint");
                let r = ui.add(
                    egui::TextEdit::singleline(&mut self.case_entry)
                        .hint_text(hint)
                        .desired_width(320.0),
                );
                if r.lost_focus() {
                    let entry = self.case_entry.clone();
                    self.state.set_case_dir(&entry);
                }
                #[cfg(windows)]
                if ui.button(self.state.tr("browse")).clicked() {
                    if let Some(d) = rfd::FileDialog::new().pick_folder() {
                        self.case_entry = d.display().to_string();
                        let entry = self.case_entry.clone();
                        self.state.set_case_dir(&entry);
                    }
                }
            });
            if self.state.out_locked() {
                ui.horizontal_wrapped(|ui| {
                    let mut on = self.state.intake_hashinv;
                    if ui
                        .checkbox(&mut on, self.state.tr("intake_hashinv"))
                        .changed()
                    {
                        self.state.set_intake_hashinv(on);
                    }
                    match &self.state.hashinv {
                        Some(Ok(t)) if self.state.intake_hashinv => {
                            ui.colored_label(GREEN, format!("rv-hashinv {}", t.version))
                                .on_hover_text(format!(
                                    "{}\nSHA-256 {}",
                                    t.path.display(),
                                    t.sha256
                                ));
                        }
                        Some(Err(e)) if self.state.intake_hashinv => {
                            ui.colored_label(RED, self.state.tr("intake_not_found"))
                                .on_hover_text(e.as_str());
                        }
                        _ => {
                            ui.weak(self.state.tr("intake_hint"));
                        }
                    }
                });
            }
            self.chat_export_block(ui);
        });
        self.preview_panel(ui);
    }

    /// Draws `add` with an amber frame when `on` (a setting the preview says must be given).
    fn highlighted<R>(ui: &mut egui::Ui, on: bool, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
        if !on {
            return add(ui);
        }
        ui.scope(|ui| {
            let w = &mut ui.visuals_mut().widgets;
            for s in [&mut w.inactive, &mut w.hovered, &mut w.active] {
                s.bg_stroke = egui::Stroke::new(2.0, AMBER);
            }
            add(ui)
        })
        .inner
    }

    fn chat_export_block(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
        ui.label(egui::RichText::new(self.state.tr("chat_export")).strong());
        let csv = self.state.show_csv_options();
        egui::Grid::new("chat-grid")
            .num_columns(2)
            .spacing([12.0, 6.0])
            .show(ui, |ui| {
                // Source
                ui.label(self.state.tr("source"));
                let mut chosen = None;
                egui::ComboBox::from_id_salt("source")
                    .selected_text(self.state.source_label(self.state.source))
                    .width(240.0)
                    .show_ui(ui, |ui| {
                        let opts =
                            std::iter::once(None).chain(SourceKind::ALL.into_iter().map(Some));
                        for k in opts {
                            if ui
                                .selectable_label(
                                    self.state.source == k,
                                    self.state.source_label(k),
                                )
                                .clicked()
                            {
                                chosen = Some(k);
                            }
                        }
                    });
                if let Some(k) = chosen {
                    self.state.set_source(k);
                }
                ui.end_row();

                // Time zone: free text + common zones
                ui.label(self.state.tr("timezone"));
                ui.horizontal(|ui| {
                    let missing = self.state.timezone_missing();
                    let hint = self.state.tr("timezone_hint");
                    let r = Self::highlighted(ui, missing, |ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut self.tz_entry)
                                .hint_text(hint)
                                .desired_width(200.0),
                        )
                    });
                    if r.lost_focus() && self.tz_entry.trim() != self.state.timezone {
                        let v = self.tz_entry.clone();
                        if let Err(e) = self.state.set_timezone(&v) {
                            self.notice = Some(e);
                        }
                    }
                    let mut pick = None;
                    egui::ComboBox::from_id_salt("tz-common")
                        .selected_text(self.state.tr("timezone_common"))
                        .show_ui(ui, |ui| {
                            for z in COMMON_ZONES {
                                if ui.selectable_label(self.state.timezone == z, z).clicked() {
                                    pick = Some(z);
                                }
                            }
                        });
                    if let Some(z) = pick {
                        self.tz_entry = z.to_string();
                        if let Err(e) = self.state.set_timezone(z) {
                            self.notice = Some(e);
                        }
                    }
                    if let Some(e) = &self.state.timezone_error {
                        ui.colored_label(RED, self.state.tr("timezone_invalid"))
                            .on_hover_text(e.as_str());
                    } else if missing {
                        ui.colored_label(AMBER, self.state.tr("required"));
                    }
                });
                ui.end_row();

                // Date order
                ui.label(self.state.tr("date_order"));
                ui.horizontal(|ui| {
                    let missing = self.state.date_order_missing();
                    let mut chosen = None;
                    Self::highlighted(ui, missing, |ui| {
                        egui::ComboBox::from_id_salt("date-order")
                            .selected_text(self.state.date_order_label(self.state.date_order))
                            .width(240.0)
                            .show_ui(ui, |ui| {
                                let opts = [
                                    None,
                                    Some(DateOrder::Dmy),
                                    Some(DateOrder::Mdy),
                                    Some(DateOrder::Ymd),
                                ];
                                for d in opts {
                                    if ui
                                        .selectable_label(
                                            self.state.date_order == d,
                                            self.state.date_order_label(d),
                                        )
                                        .clicked()
                                    {
                                        chosen = Some(d);
                                    }
                                }
                            });
                    });
                    if let Some(d) = chosen {
                        self.state.set_date_order(d);
                    }
                    if missing {
                        ui.colored_label(AMBER, self.state.tr("required"));
                    }
                });
                ui.end_row();

                // Custodian: free text + participants found by the preview
                ui.label(self.state.tr("custodian"));
                ui.horizontal(|ui| {
                    let hint = self.state.tr("custodian_hint");
                    let r = ui.add(
                        egui::TextEdit::singleline(&mut self.custodian_entry)
                            .hint_text(hint)
                            .desired_width(200.0),
                    );
                    if r.lost_focus() {
                        let v = self.custodian_entry.clone();
                        self.state.set_custodian(&v);
                    }
                    if !self.state.known_participants.is_empty() {
                        let mut pick = None;
                        egui::ComboBox::from_id_salt("custodian-found")
                            .selected_text(self.state.tr("custodian_found"))
                            .show_ui(ui, |ui| {
                                for p in &self.state.known_participants {
                                    if ui.selectable_label(self.state.custodian == *p, p).clicked()
                                    {
                                        pick = Some(p.clone());
                                    }
                                }
                            });
                        if let Some(p) = pick {
                            self.custodian_entry = p.clone();
                            self.state.set_custodian(&p);
                        }
                    }
                });
                ui.end_row();

                if csv {
                    ui.label(self.state.tr("csv_profile"));
                    let mut chosen = None;
                    egui::ComboBox::from_id_salt("csv-profile")
                        .selected_text(self.state.csv_profile_label(self.state.csv_profile))
                        .width(240.0)
                        .show_ui(ui, |ui| {
                            for p in [
                                CsvProfile::Generic,
                                CsvProfile::XryDraft,
                                CsvProfile::CellebriteDraft,
                            ] {
                                if ui
                                    .selectable_label(
                                        self.state.csv_profile == p,
                                        self.state.csv_profile_label(p),
                                    )
                                    .clicked()
                                {
                                    chosen = Some(p);
                                }
                            }
                        });
                    if let Some(p) = chosen {
                        self.state.set_csv_profile(p);
                    }
                    ui.end_row();
                    ui.label(self.state.tr("csv_map"));
                    ui.horizontal(|ui| {
                        let hint = self.state.tr("csv_map_hint");
                        let r = ui.add(
                            egui::TextEdit::singleline(&mut self.csv_map_entry)
                                .hint_text(hint)
                                .desired_width(280.0),
                        );
                        if r.lost_focus() {
                            let v = self.csv_map_entry.clone();
                            self.state.set_csv_map(&v);
                        }
                        #[cfg(windows)]
                        if ui.button(self.state.tr("browse")).clicked() {
                            if let Some(f) = rfd::FileDialog::new()
                                .add_filter("TOML", &["toml"])
                                .pick_file()
                            {
                                self.csv_map_entry = f.display().to_string();
                                let v = self.csv_map_entry.clone();
                                self.state.set_csv_map(&v);
                            }
                        }
                    });
                    ui.end_row();
                }

                ui.label(self.state.tr("platform"));
                let hint = self.state.tr("platform_hint");
                let r = ui.add(
                    egui::TextEdit::singleline(&mut self.platform_entry)
                        .hint_text(hint)
                        .desired_width(280.0),
                );
                if r.lost_focus() {
                    let v = self.platform_entry.clone();
                    self.state.set_platform_name(&v);
                }
                ui.end_row();
            });
        if ui
            .add_enabled(
                !self.state.inputs.is_empty(),
                egui::Button::new(self.state.tr("preview")),
            )
            .on_hover_text(self.state.tr("preview_tip"))
            .clicked()
        {
            self.state.preview();
        }
    }

    fn preview_panel(&mut self, ui: &mut egui::Ui) {
        let Some(p) = self.state.preview.clone() else {
            return;
        };
        egui::CollapsingHeader::new(format!(
            "{} ({} {})",
            self.state.tr("preview_title"),
            p.exports.len(),
            self.state.tr("exports")
        ))
        .id_salt("preview-panel")
        .default_open(true)
        .show(ui, |ui| {
            if let Some(e) = &p.input_error {
                ui.colored_label(
                    AMBER,
                    format!("{}: {e}", self.state.tr("preview_input_error")),
                );
            }
            if let Some(u) = &p.usage {
                ui.colored_label(AMBER, self.state.usage_help().unwrap_or_default());
                ui.weak(u.path.display().to_string());
                if !self.state.date_order_evidence.is_empty()
                    && matches!(
                        u.error,
                        rv_rsmf_core::sources::ParseError::NeedsDateOrder { .. }
                    )
                {
                    ui.label(self.state.tr("needs_date_order_evidence"));
                    for l in &self.state.date_order_evidence {
                        ui.monospace(l);
                    }
                }
            } else if p.exports.is_empty() && p.input_error.is_none() {
                ui.colored_label(AMBER, self.state.tr("preview_none"));
            }
            for (i, e) in p.exports.iter().enumerate() {
                ui.separator();
                ui.label(
                    egui::RichText::new(format!("{}  [{}]", e.path, e.source.name())).strong(),
                );
                if let Some(err) = &e.error {
                    ui.colored_label(AMBER, self.state.tr("preview_failed"));
                    ui.weak(err.as_str());
                    continue;
                }
                ui.label(format!(
                    "{} {} \u{00B7} {} {} \u{00B7} {} {}",
                    e.conversations,
                    self.state.tr("conversations"),
                    e.participants.len(),
                    self.state.tr("participants"),
                    e.events,
                    self.state.tr("events"),
                ));
                for (k, v) in &e.facts {
                    ui.weak(format!("{k}: {v}"));
                }
                egui::ScrollArea::both()
                    .max_height(180.0)
                    .id_salt(("preview-rows", i))
                    .show(ui, |ui| {
                        egui::Grid::new(("preview-grid", i))
                            .striped(true)
                            .num_columns(4)
                            .spacing([12.0, 2.0])
                            .show(ui, |ui| {
                                for h in ["col_written", "col_utc", "col_from", "col_text"] {
                                    ui.label(egui::RichText::new(self.state.tr(h)).color(CYAN));
                                }
                                ui.end_row();
                                for r in &e.first {
                                    ui.monospace(&r.source_time);
                                    ui.monospace(&r.utc);
                                    ui.label(&r.sender);
                                    ui.label(&r.body);
                                    ui.end_row();
                                }
                            });
                    });
            }
        });
    }

    fn hashing_section(&mut self, ui: &mut egui::Ui, enabled: bool) {
        Self::section_title(ui, self.state.tr("hashes"));
        ui.add_enabled_ui(enabled, |ui| {
            ui.horizontal_wrapped(|ui| {
                for a in HashAlgo::ALL {
                    let mut on = self.state.selected.contains(&a);
                    let tip = if self.state.can_toggle(a) {
                        String::new()
                    } else if on {
                        self.state.tr("hash_tip_one")
                    } else {
                        self.state.tr("hash_tip_two")
                    };
                    let r = ui.add_enabled(
                        self.state.can_toggle(a),
                        egui::Checkbox::new(&mut on, a.name()),
                    );
                    let r = if tip.is_empty() {
                        r
                    } else {
                        r.on_disabled_hover_text(tip)
                    };
                    if r.changed()
                        && let Err(e) = self.state.toggle_algo(a)
                    {
                        self.notice = Some(e);
                    }
                }
            });
        });
    }

    fn report_section(&mut self, ui: &mut egui::Ui, enabled: bool) {
        Self::section_title(ui, self.state.tr("reports"));
        let tr = |k: &str| self.state.tr(k);
        let (l_lang, l_out, l_browse, l_exam, l_case, l_hint, l_all) = (
            tr("report_lang"),
            tr("out_dir"),
            tr("browse"),
            tr("examiner"),
            tr("case"),
            tr("case_hint"),
            tr("lang_all"),
        );
        let l_locked = tr("out_from_case");
        ui.add_enabled_ui(enabled, |ui| {
            egui::Grid::new("report-grid")
                .num_columns(2)
                .spacing([12.0, 6.0])
                .show(ui, |ui| {
                    ui.label(l_lang);
                    let choices = [
                        ("en", "English".to_string()),
                        ("es", "Español".to_string()),
                        ("fr", "Français".to_string()),
                        ("all", l_all),
                    ];
                    let current = choices
                        .iter()
                        .find(|(k, _)| *k == self.state.lang)
                        .map(|(_, v)| v.clone())
                        .unwrap_or_default();
                    let mut chosen = None;
                    egui::ComboBox::from_id_salt("lang")
                        .selected_text(current)
                        .show_ui(ui, |ui| {
                            for (k, v) in &choices {
                                if ui.selectable_label(self.state.lang == *k, v).clicked() {
                                    chosen = Some(*k);
                                }
                            }
                        });
                    if let Some(k) = chosen {
                        self.state.set_lang(k);
                    }
                    ui.end_row();
                    ui.label(l_out);
                    ui.horizontal(|ui| {
                        if self.state.out_locked() {
                            // Rule (state.rs): the case folder decides where the run is written.
                            let mut shown = self.state.out_dir_display();
                            ui.add_enabled(
                                false,
                                egui::TextEdit::singleline(&mut shown).desired_width(300.0),
                            )
                            .on_disabled_hover_text(&l_locked);
                        } else {
                            ui.add(
                                egui::TextEdit::singleline(&mut self.state.out_dir)
                                    .desired_width(300.0),
                            );
                            #[cfg(windows)]
                            if ui.button(&l_browse).clicked() {
                                if let Some(d) = rfd::FileDialog::new().pick_folder() {
                                    self.state.out_dir = d.display().to_string();
                                }
                            }
                        }
                        let _ = &l_browse;
                    });
                    ui.end_row();
                    ui.label(l_exam);
                    ui.add(egui::TextEdit::singleline(&mut self.state.actor).desired_width(300.0));
                    ui.end_row();
                    ui.label(l_case);
                    ui.add(
                        egui::TextEdit::singleline(&mut self.state.case_id)
                            .hint_text(l_hint)
                            .desired_width(300.0),
                    );
                    ui.end_row();
                });
        });
    }

    fn trust_section(&mut self, ui: &mut egui::Ui, enabled: bool) {
        Self::section_title(ui, self.state.tr("trust"));
        let tr = |k: &str| self.state.tr(k);
        let (l_jur, l_co, l_fr, l_tsa, l_cert, l_pass, l_browse) = (
            tr("jurisdiction"),
            tr("jur_co"),
            tr("jur_fr"),
            tr("tsa"),
            tr("sign_cert"),
            tr("sign_pass"),
            tr("browse"),
        );
        ui.add_enabled_ui(enabled, |ui| {
            egui::Grid::new("trust-grid")
                .num_columns(2)
                .spacing([12.0, 6.0])
                .show(ui, |ui| {
                    ui.label(l_jur);
                    let current = if self.state.jurisdiction == "fr" {
                        l_fr.clone()
                    } else {
                        l_co.clone()
                    };
                    let mut chosen = None;
                    egui::ComboBox::from_id_salt("jur")
                        .selected_text(current)
                        .show_ui(ui, |ui| {
                            if ui
                                .selectable_label(self.state.jurisdiction == "co", &l_co)
                                .clicked()
                            {
                                chosen = Some("co");
                            }
                            if ui
                                .selectable_label(self.state.jurisdiction == "fr", &l_fr)
                                .clicked()
                            {
                                chosen = Some("fr");
                            }
                        });
                    if let Some(j) = chosen {
                        self.state.set_jurisdiction(j);
                    }
                    ui.end_row();
                    ui.label(l_tsa);
                    ui.horizontal(|ui| {
                        let mut on = self.state.timestamp;
                        let mut url = self.state.tsa_url.clone();
                        let c = ui.checkbox(&mut on, "");
                        let t = ui.add_enabled(
                            on,
                            egui::TextEdit::singleline(&mut url).desired_width(280.0),
                        );
                        if c.changed() || t.lost_focus() {
                            self.state.set_timestamp(on, &url);
                        } else {
                            self.state.tsa_url = url;
                        }
                    });
                    ui.end_row();
                    ui.label(l_cert);
                    ui.horizontal(|ui| {
                        let mut path = self.state.sign_cert.clone();
                        let r = ui.add(egui::TextEdit::singleline(&mut path).desired_width(280.0));
                        if r.lost_focus() || r.changed() {
                            self.state.set_sign_cert(&path);
                        }
                        #[cfg(windows)]
                        if ui.button(&l_browse).clicked() {
                            if let Some(f) = rfd::FileDialog::new()
                                .add_filter("PKCS#12", &["p12", "pfx"])
                                .pick_file()
                            {
                                self.state.set_sign_cert(&f.display().to_string());
                            }
                        }
                        let _ = &l_browse;
                    });
                    ui.end_row();
                    ui.label(l_pass);
                    ui.add(
                        egui::TextEdit::singleline(&mut self.state.sign_password)
                            .password(true)
                            .desired_width(280.0),
                    );
                    ui.end_row();
                });
        });
    }

    fn run_section(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if self.state.is_running() {
                if ui.button(self.state.tr("cancel")).clicked() {
                    self.cancel.store(true, Ordering::SeqCst);
                    self.notice = Some(self.state.tr("cancelling"));
                }
            } else {
                let start = egui::Button::new(
                    egui::RichText::new(self.state.tr("start"))
                        .strong()
                        .color(egui::Color32::WHITE),
                )
                .fill(BLUE);
                if ui.add_enabled(self.state.can_start(), start).clicked() {
                    let ctx = ui.ctx().clone();
                    self.start(&ctx);
                }
            }
            if let Some(n) = &self.notice {
                ui.colored_label(AMBER, n);
            }
        });
        ui.add_space(6.0);
        ui.add(
            egui::ProgressBar::new(self.state.fraction())
                .text(self.state.progress_text())
                .fill(BLUE)
                .animate(self.state.is_running()),
        );
        let live = self.started.map(|s| s.elapsed().as_millis() as u64);
        ui.label(self.state.timing_text(live));
        if let Some((done, total)) = self.state.intake_progress {
            ui.weak(format!(
                "{}: {done}/{total} {}",
                self.state.tr("intake_progress"),
                self.state.tr("files")
            ));
        }
        if let Some(t) = self.state.conversion_text() {
            ui.weak(t);
        }
        if !self.state.current_item.is_empty() {
            ui.weak(format!(
                "{}: {}",
                self.state.tr("now"),
                self.state.current_item
            ));
        }
        match &self.state.phase {
            Phase::Failed(e) => {
                ui.colored_label(RED, format!("{}: {}", self.state.tr("failed"), e));
            }
            Phase::Finished | Phase::Cancelled => {
                if let Some(o) = self.state.outcome.clone() {
                    ui.separator();
                    let title = if self.state.phase == Phase::Cancelled {
                        self.state.tr("cancelled")
                    } else {
                        self.state.tr("done")
                    };
                    ui.label(egui::RichText::new(title).color(CYAN).strong());
                    ui.label(format!(
                        "{} {} \u{00B7} {} {} \u{00B7} {}: {} / {} / {} / {}",
                        o.items,
                        self.state.tr("files"),
                        o.errors,
                        self.state.tr("errors"),
                        self.state.tr("findings"),
                        o.high,
                        o.medium,
                        o.low,
                        o.info
                    ));
                    let c = &o.conversion;
                    ui.horizontal_wrapped(|ui| {
                        ui.label(format!(
                            "{} {} \u{00B7} {} {} \u{00B7} {} {} \u{00B7} {} {}",
                            c.exports,
                            self.state.tr("exports"),
                            c.conversations,
                            self.state.tr("conversations"),
                            c.events,
                            self.state.tr("events"),
                            c.files,
                            self.state.tr("rsmf_files"),
                        ));
                        if c.files_invalid > 0 {
                            ui.colored_label(
                                RED,
                                format!("({} {})", c.files_invalid, self.state.tr("invalid")),
                            );
                        }
                        if c.exports_failed > 0 {
                            ui.colored_label(
                                RED,
                                format!(
                                    "({} {})",
                                    c.exports_failed,
                                    self.state.tr("exports_failed")
                                ),
                            );
                        }
                    });
                    let lines = self.state.evidence_lines();
                    if !lines.is_empty() {
                        egui::CollapsingHeader::new(format!(
                            "{} ({})",
                            self.state.tr("evidence_ids"),
                            lines.len()
                        ))
                        .default_open(true)
                        .show(ui, |ui| {
                            egui::ScrollArea::vertical()
                                .max_height(120.0)
                                .id_salt("evidence-ids")
                                .show(ui, |ui| {
                                    for l in &lines {
                                        ui.monospace(l);
                                    }
                                });
                        });
                    }
                    ui.horizontal(|ui| {
                        if ui.button(self.state.tr("open_report")).clicked() {
                            open_path(&o.report);
                        }
                        if ui.button(self.state.tr("open_logs")).clicked() {
                            open_path(&o.log_dir);
                        }
                        if let Some(dir) = &o.rsmf_dir
                            && ui.button(self.state.tr("open_rsmf")).clicked()
                        {
                            open_path(dir);
                        }
                        if let Some(dir) = self.state.intake_report_dir.clone()
                            && ui.button(self.state.tr("open_hash_report")).clicked()
                        {
                            let lang = if self.state.lang == "all" {
                                "en"
                            } else {
                                self.state.lang.as_str()
                            };
                            let stats = Path::new(&dir).join(format!("statistics.{lang}.html"));
                            open_path(&if stats.is_file() {
                                stats
                            } else {
                                PathBuf::from(dir)
                            });
                        }
                    });
                }
            }
            _ => {}
        }
        if !self.state.warnings.is_empty() {
            egui::CollapsingHeader::new(format!(
                "{} ({})",
                self.state.tr("warnings"),
                self.state.warnings.len()
            ))
            .show(ui, |ui| {
                for w in &self.state.warnings {
                    ui.label(w);
                }
            });
        }
    }
}

impl Default for HashInvApp {
    fn default() -> Self {
        Self::new()
    }
}

fn open_path(p: &Path) {
    #[cfg(windows)]
    let _ = std::process::Command::new("explorer").arg(p).spawn();
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(p).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let _ = std::process::Command::new("xdg-open").arg(p).spawn();
}

impl eframe::App for HashInvApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if !self.themed {
            apply_theme(ui.ctx());
            self.themed = true;
        }
        self.drain();
        if self.state.is_running() {
            // Keep the elapsed timer moving even when no event arrives (large files).
            ui.ctx().request_repaint_after(Duration::from_millis(200));
        }
        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                self.header(ui);
                let enabled = !self.state.is_running();
                self.evidence_section(ui, enabled);
                self.hashing_section(ui, enabled);
                self.report_section(ui, enabled);
                self.trust_section(ui, enabled);
                self.run_section(ui);
            });
        });
    }
}
