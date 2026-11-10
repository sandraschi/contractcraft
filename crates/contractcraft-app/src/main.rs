//! `contractcraft-app`: Native eframe/egui desktop workstation for ContractCraft.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use craft_biz::bus::Bus;
use craft_biz::confine::Confinement;
use contractcraft_core::{dispatch, Command, Ctx};
use contractcraft_store::{Contract, Contracts, Seal, Status, Version};
use eframe::egui::{self, Color32, RichText};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ViewFilter {
    All,
    Draft,
    UnderReview,
    PendingSignature,
    Executed,
    Terminated,
}

struct App {
    ctx: Ctx,
    view: ViewFilter,
    contracts: Vec<Contract>,
    selected_contract: Option<Contract>,
    selected_version: Option<Version>,
    selected_seals: Vec<Seal>,
    status_msg: String,
}

impl App {
    fn new(ctx: Ctx) -> Self {
        let mut app = Self {
            ctx,
            view: ViewFilter::All,
            contracts: Vec::new(),
            selected_contract: None,
            selected_version: None,
            selected_seals: Vec::new(),
            status_msg: "Ready".to_string(),
        };
        app.reload_contracts();
        app
    }

    fn reload_contracts(&mut self) {
        let status_filter = match self.view {
            ViewFilter::All => None,
            ViewFilter::Draft => Some(Status::Draft),
            ViewFilter::UnderReview => Some(Status::UnderReview),
            ViewFilter::PendingSignature => Some(Status::PendingSignature),
            ViewFilter::Executed => Some(Status::Executed),
            ViewFilter::Terminated => Some(Status::Terminated),
        };

        if let Ok(cs) = self.ctx.db.contracts(status_filter, None) {
            self.contracts = cs;
            if self.selected_contract.is_none() && !self.contracts.is_empty() {
                let first = self.contracts[0].clone();
                self.select_contract(first);
            }
        }
    }

    fn select_contract(&mut self, c: Contract) {
        if let Ok(v) = self.ctx.db.version(&c, None) {
            self.selected_version = Some(v);
        }
        if let Ok(seals) = self.ctx.db.seals(&c) {
            self.selected_seals = seals;
        }
        self.selected_contract = Some(c);
    }

    fn transition_selected(&mut self, to: Status) {
        let Some(ref c) = self.selected_contract else {
            return;
        };

        match dispatch(
            &mut self.ctx,
            Command::Transition {
                contract: c.number.clone(),
                to,
                note: format!("Moved to {} via workstation", to.as_str()),
            },
        ) {
            Ok(_) => {
                self.status_msg = format!("Transitioned {} -> {}", c.number, to.as_str());
                if let Ok(updated) = self.ctx.db.contract(&c.number) {
                    self.select_contract(updated);
                }
                self.reload_contracts();
            }
            Err(e) => {
                self.status_msg = format!("Transition refused: {e}");
            }
        }
    }

    fn seal_selected(&mut self) {
        let Some(ref c) = self.selected_contract else {
            return;
        };

        match dispatch(
            &mut self.ctx,
            Command::Seal {
                contract: c.number.clone(),
            },
        ) {
            Ok(_) => {
                self.status_msg = format!("Cryptographically sealed {}", c.number);
                if let Ok(updated) = self.ctx.db.contract(&c.number) {
                    self.select_contract(updated);
                }
            }
            Err(e) => {
                self.status_msg = format!("Seal failed: {e}");
            }
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        ui.horizontal(|ui| {
            ui.heading(
                RichText::new("📜 ContractCraft CLM Studio")
                    .color(Color32::from_rgb(56, 189, 248))
                    .strong(),
            );
            ui.separator();

            if ui
                .selectable_label(self.view == ViewFilter::All, "📋 All")
                .clicked()
            {
                self.view = ViewFilter::All;
                self.reload_contracts();
            }
            if ui
                .selectable_label(self.view == ViewFilter::Draft, "📝 Draft")
                .clicked()
            {
                self.view = ViewFilter::Draft;
                self.reload_contracts();
            }
            if ui
                .selectable_label(self.view == ViewFilter::UnderReview, "🔍 Review")
                .clicked()
            {
                self.view = ViewFilter::UnderReview;
                self.reload_contracts();
            }
            if ui
                .selectable_label(self.view == ViewFilter::PendingSignature, "✍️ Signature")
                .clicked()
            {
                self.view = ViewFilter::PendingSignature;
                self.reload_contracts();
            }
            if ui
                .selectable_label(self.view == ViewFilter::Executed, "✅ Executed")
                .clicked()
            {
                self.view = ViewFilter::Executed;
                self.reload_contracts();
            }
            if ui
                .selectable_label(self.view == ViewFilter::Terminated, "🛑 Terminated")
                .clicked()
            {
                self.view = ViewFilter::Terminated;
                self.reload_contracts();
            }

            ui.separator();
            if ui.button("🔄 Refresh").clicked() {
                self.reload_contracts();
                self.status_msg = "Contracts reloaded".to_string();
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(format!("Actor: {}", self.ctx.actor));
            });
        });

        ui.separator();

        ui.horizontal(|ui| {
            // Left list of contracts
            ui.vertical(|ui| {
                ui.set_width(340.0);
                ui.heading(RichText::new("Contract Pipeline").size(15.0));
                ui.separator();

                let mut to_select = None;
                egui::ScrollArea::vertical()
                    .id_salt("contract_list_scroll")
                    .show(ui, |ui| {
                        for c in &self.contracts {
                            let is_sel = self
                                .selected_contract
                                .as_ref()
                                .map(|sc| sc.id == c.id)
                                .unwrap_or(false);

                            let st_color = match c.status {
                                Status::Draft => Color32::from_rgb(149, 165, 166),
                                Status::UnderReview => Color32::from_rgb(241, 196, 15),
                                Status::PendingSignature => Color32::from_rgb(230, 126, 34),
                                Status::Executed => Color32::from_rgb(46, 204, 113),
                                Status::Terminated | Status::Expired | Status::Cancelled => {
                                    Color32::from_rgb(231, 76, 60)
                                }
                            };

                            let is_sealed = self.ctx.db.seals(c).map(|s| !s.is_empty()).unwrap_or(false);

                            ui.group(|ui| {
                                if ui
                                    .selectable_label(
                                        is_sel,
                                        format!("{}: {}", c.number, c.title),
                                    )
                                    .clicked()
                                {
                                    to_select = Some(c.clone());
                                }
                                ui.horizontal(|ui| {
                                    ui.colored_label(st_color, format!("[{}]", c.status.as_str()));
                                    ui.label(&c.counterparty);
                                    if is_sealed {
                                        ui.colored_label(
                                            Color32::from_rgb(52, 152, 219),
                                            "🔒 SEALED",
                                        );
                                    }
                                });
                            });
                            ui.add_space(2.0);
                        }
                    });

                if let Some(c) = to_select {
                    self.select_contract(c);
                }
            });

            ui.separator();

            // Right detail
            ui.vertical(|ui| {
                if let Some(ref c) = self.selected_contract {
                    ui.horizontal(|ui| {
                        ui.heading(
                            RichText::new(format!("{}: {}", c.number, c.title)).size(18.0),
                        );
                        ui.separator();
                        ui.label(format!("Counterparty: {}", c.counterparty));
                        ui.label(format!("Version: v{}", c.current_version));
                    });

                    let mut transition_to = None;
                    let mut do_seal = false;
                    let next_statuses = c.status.next();
                    ui.horizontal(|ui| {
                        ui.label("Lifecycle Action:");
                        for next_st in next_statuses {
                            if ui.button(format!("Move to {}", next_st.as_str())).clicked() {
                                transition_to = Some(next_st);
                            }
                        }
                        if ui.button("🔒 Cryptographic Seal").clicked() {
                            do_seal = true;
                        }
                    });
                    if let Some(target_st) = transition_to {
                        self.transition_selected(target_st);
                    }
                    if do_seal {
                        self.seal_selected();
                    }

                    ui.separator();

                    if !self.selected_seals.is_empty() {
                        ui.horizontal(|ui| {
                            ui.colored_label(
                                Color32::from_rgb(46, 204, 113),
                                format!("✅ Cryptographically Sealed ({} seal(s))", self.selected_seals.len()),
                            );
                            if let Some(last) = self.selected_seals.last() {
                                ui.label(format!("Hash: {}", &last.sha256[..16]));
                            }
                        });
                        ui.separator();
                    }

                    ui.label(RichText::new("Contract Legal Text:").strong());
                    if let Some(ref v) = self.selected_version {
                        egui::ScrollArea::vertical()
                            .id_salt("contract_body_scroll")
                            .max_height(400.0)
                            .show(ui, |ui| {
                                ui.add(
                                    egui::TextEdit::multiline(&mut v.body.clone())
                                        .font(egui::TextStyle::Monospace)
                                        .desired_width(f32::INFINITY)
                                        .interactive(false),
                                );
                            });
                    }
                } else {
                    ui.centered_and_justified(|ui| {
                        ui.label("Select a contract from the pipeline to view.");
                    });
                }
            });
        });

        ui.separator();
        ui.horizontal(|ui| {
            ui.label(format!(
                "Contracts: {} | Selected: {}",
                self.contracts.len(),
                self.selected_contract.as_ref().map(|c| c.number.as_str()).unwrap_or("-")
            ));
            ui.separator();
            ui.label(&self.status_msg);
        });
    }
}

fn default_db() -> PathBuf {
    if let Ok(p) = std::env::var("CONTRACTCRAFT_DB") {
        if !p.trim().is_empty() {
            return PathBuf::from(p);
        }
    }
    let base = std::env::var("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."));
    base.join("contractcraft").join("contractcraft.db")
}

fn default_key() -> PathBuf {
    let base = std::env::var("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."));
    base.join("contractcraft").join("seal.key")
}

fn main() -> eframe::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--smoke") {
        println!("smoke: ok (contractcraft-app)");
        return Ok(());
    }

    let db_path = default_db();
    if let Some(parent) = db_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let key_path = default_key();
    if let Some(parent) = key_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let db = match Contracts::open(&db_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Failed to open contracts database at {}: {e}", db_path.display());
            std::process::exit(1);
        }
    };

    let ctx = Ctx {
        db,
        confinement: Confinement(None),
        bus: Bus::new(Bus::default_root()),
        key_path,
        actor: std::env::var("USERNAME")
            .or_else(|_| std::env::var("USER"))
            .unwrap_or_else(|_| "desktop".to_string()),
    };

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1200.0, 760.0])
            .with_min_inner_size([800.0, 500.0])
            .with_title("ContractCraft CLM Studio"),
        ..Default::default()
    };

    eframe::run_native(
        "ContractCraft",
        native_options,
        Box::new(|_cc| Ok(Box::new(App::new(ctx)))),
    )
}
