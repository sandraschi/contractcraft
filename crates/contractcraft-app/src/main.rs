//! ContractCraft desktop window, laid out like a contract lifecycle tool:
//! pipeline board by status, contract list, contract page (text with risk
//! highlights, versions with inline redline, risks, seals, terms and next
//! deadline, history), deadlines, templates, a free-text risk check.
//!
//! Every change goes through `contractcraft_core::dispatch` (same path as
//! CLI and MCP). Reads use the store, `risk::scan` and `next_deadline`.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use chrono::{Local, NaiveDate};
use contractcraft_core::{dispatch, next_deadline, risk, tools, Command, Confinement, Ctx};
use contractcraft_store::{Contract, Contracts, Seal, Status, Template, Transition, Version};
use craft_biz::bus::Bus;
use craft_biz::Money;
use craft_biz_ui::app::{self, Frame, NavEntry, SuiteApp, Wiring};
use craft_biz_ui::kanban::{self, Board, Card, Column};
use craft_biz_ui::table::{Cell, Col, Table, TableEvent};
use craft_biz_ui::theme::{self, semibold};
use craft_biz_ui::widgets::{self, Kpi};
use craft_biz_ui::Tone;
use egui::{RichText, Ui};
use serde_json::{json, Value};
use similar::{ChangeTag, TextDiff};
use std::path::{Path, PathBuf};

const V_PIPELINE: usize = 0;
const V_LIST: usize = 1;
const V_DEADLINES: usize = 2;
const V_TEMPLATES: usize = 3;
const V_RISK: usize = 4;
const V_SETTINGS: usize = 5;

/// Every MCP tool and where the window performs it (checked by a test).
pub const COVERAGE: &[(&str, &str)] = &[
    ("contract_status", "Settings > Status; pipeline counts"),
    ("contract_template_add", "Templates > New template"),
    ("contract_template_list", "Templates view"),
    ("contract_template_show", "Templates > Show"),
    (
        "contract_draft",
        "Top bar + Contract; Templates > Use template",
    ),
    ("contract_list", "Pipeline board; All contracts table"),
    ("contract_show", "Contract page > Full record"),
    (
        "contract_edit",
        "Contract page > Text > Save as new version",
    ),
    ("contract_terms", "Contract page > Terms > Edit terms"),
    (
        "contract_transition",
        "Pipeline drag; Contract page status buttons",
    ),
    (
        "contract_seal",
        "Contract page > Seals > Seal current version",
    ),
    (
        "contract_verify",
        "Contract page > Seals > Verify, Verify a file",
    ),
    (
        "contract_redline",
        "Contract page > Versions > Redline report",
    ),
    (
        "contract_risk_scan",
        "Risk check view; Contract page > Risks > Full scan",
    ),
    ("contract_render", "Contract page > Export (md, typ, pdf)"),
    ("contract_deadlines", "Deadlines view; Send to taskcraft"),
    ("contract_drain_inbox", "Settings > Take new hires"),
    ("contract_demo_seed", "Empty state; Settings > Demo data"),
];

/// Per board column: why a card may not be dropped there (None = allowed).
type Refusals = Vec<(String, Option<String>)>;

struct Row {
    c: Contract,
    sealed_current: bool,
    deadline: Option<Value>,
    findings: usize,
    worst: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Text,
    Versions,
    Risks,
    Seals,
    Terms,
    History,
}

struct Page {
    key: String,
    body: String,
    versions: Vec<Version>,
    seals: Vec<Seal>,
    history: Vec<Transition>,
    findings: Vec<risk::Finding>,
    tab: Tab,
    edit: Option<(String, String)>,
    /// View the page was opened from; leaving that view closes the page.
    view: Option<usize>,
    from: i64,
    to: i64,
}

pub struct ContractApp {
    ctx: Ctx,
    db: String,
    today: NaiveDate,
    rows: Vec<Row>,
    templates: Vec<Template>,
    rules: Vec<risk::Rule>,
    page: Option<Page>,
    risk_text: String,
    risk_out: Option<Vec<risk::Finding>>,
}

fn eur(c: i64) -> String {
    Money::from_cents(c).format_de()
}

/// Annual value cell; a missing value is a plain weak dash, not a red money figure.
fn money_cell(cents: Option<i64>) -> Cell {
    match cents {
        Some(c) => Cell::Money(eur(c)),
        None => Cell::Weak("-".into()),
    }
}

fn status_tone(s: Status) -> Tone {
    match s {
        Status::Draft => Tone::Neutral,
        Status::UnderReview => Tone::Info,
        Status::PendingSignature => Tone::Warn,
        Status::Executed => Tone::Ok,
        Status::Terminated | Status::Expired => Tone::Violet,
        Status::Cancelled => Tone::Danger,
    }
}

fn status_label(s: Status, de: bool) -> &'static str {
    match (s, de) {
        (Status::Draft, true) => "Entwurf",
        (Status::Draft, false) => "Draft",
        (Status::UnderReview, true) => "In Prüfung",
        (Status::UnderReview, false) => "Under review",
        (Status::PendingSignature, true) => "Zur Unterschrift",
        (Status::PendingSignature, false) => "Pending signature",
        (Status::Executed, true) => "Unterzeichnet",
        (Status::Executed, false) => "Executed",
        (Status::Terminated, true) => "Gekündigt",
        (Status::Terminated, false) => "Terminated",
        (Status::Expired, true) => "Abgelaufen",
        (Status::Expired, false) => "Expired",
        (Status::Cancelled, true) => "Abgebrochen",
        (Status::Cancelled, false) => "Cancelled",
    }
}

fn sev_tone(s: &str) -> Tone {
    match s {
        "high" => Tone::Danger,
        "medium" => Tone::Warn,
        _ => Tone::Info,
    }
}

fn sev_rank(s: &str) -> u8 {
    match s {
        "high" => 3,
        "medium" => 2,
        _ => 1,
    }
}

fn date_de(d: NaiveDate) -> String {
    d.format("%d.%m.%Y").to_string()
}

fn ts(s: &str) -> String {
    s.get(..16).unwrap_or(s).replace('T', " ")
}

/// Board column of a status (ended states share one column).
fn column_of(s: Status) -> &'static str {
    match s {
        Status::Draft => "draft",
        Status::UnderReview => "under_review",
        Status::PendingSignature => "pending_signature",
        Status::Executed => "executed",
        _ => "ended",
    }
}

/// Target status of a drop into a column.
fn target_of(from: Status, col: &str) -> Option<Status> {
    match col {
        "draft" => Some(Status::Draft),
        "under_review" => Some(Status::UnderReview),
        "pending_signature" => Some(Status::PendingSignature),
        "executed" => Some(Status::Executed),
        "ended" => Some(if from == Status::Executed {
            Status::Terminated
        } else {
            Status::Cancelled
        }),
        _ => None,
    }
}

impl ContractApp {
    pub fn new(ctx: Ctx, db: String) -> ContractApp {
        let rules = risk::default_rules().unwrap_or_default();
        ContractApp {
            ctx,
            db,
            today: Local::now().date_naive(),
            rows: Vec::new(),
            templates: Vec::new(),
            rules,
            page: None,
            risk_text: String::new(),
            risk_out: None,
        }
    }

    fn row(&self, key: &str) -> Option<&Row> {
        self.rows.iter().find(|r| r.c.number == key)
    }

    fn open(&mut self, key: &str) {
        let r = (|| -> Result<Page, String> {
            let db = &self.ctx.db;
            let c = db.contract(key).map_err(|e| e.to_string())?;
            let body = db.version(&c, None).map_err(|e| e.to_string())?.body;
            let versions = db.versions(&c).map_err(|e| e.to_string())?;
            let findings = risk::scan(&self.rules, &body);
            let (tab, edit, from, to, view) = match &self.page {
                Some(p) if p.key == c.number => (p.tab, p.edit.clone(), p.from, p.to, p.view),
                _ => (
                    Tab::Text,
                    None,
                    (c.current_version - 1).max(1),
                    c.current_version,
                    None,
                ),
            };
            Ok(Page {
                key: c.number.clone(),
                body,
                seals: db.seals(&c).map_err(|e| e.to_string())?,
                history: db.history(&c).map_err(|e| e.to_string())?,
                versions,
                findings,
                tab,
                edit,
                from: from.min(c.current_version),
                to: to.min(c.current_version),
                view,
            })
        })();
        match r {
            Ok(p) => self.page = Some(p),
            Err(e) => eprintln!("contractcraft-app: {e}"),
        }
    }

    /// Why a move is not allowed, in words (None = allowed).
    fn refuse(&self, r: &Row, to: Status, de: bool) -> Option<String> {
        if !r.c.status.can_move_to(to) {
            return Some(if de {
                format!(
                    "{} -> {} ist kein erlaubter Schritt",
                    status_label(r.c.status, true),
                    status_label(to, true)
                )
            } else {
                format!(
                    "{} -> {} is not an allowed step",
                    status_label(r.c.status, false),
                    status_label(to, false)
                )
            });
        }
        if to == Status::Executed && !r.sealed_current {
            return Some(if de {
                format!("Erst Version {} versiegeln", r.c.current_version)
            } else {
                format!("Seal version {} first", r.c.current_version)
            });
        }
        None
    }

    fn move_to(&self, f: &mut Frame, key: &str, to: Status) {
        let de = f.lang_de;
        let label = format!("{key} -> {}", status_label(to, de));
        let args = json!({ "contract": key, "to": to.as_str() });
        match to {
            Status::Executed => f.run_confirm(
                if de { format!("{key} als unterzeichnet markieren? Danach sind Text und Bedingungen fix, gaincraft erhält ARR und Fristen.") } else { format!("Mark {key} as executed? Text and terms are fixed afterwards and gaincraft receives ARR and dates.") },
                label,
                "contract_transition",
                args,
            ),
            Status::Terminated | Status::Cancelled | Status::Expired => f.run_confirm(if de { format!("{key}: {} - das lässt sich nicht rückgängig machen.", status_label(to, true)) } else { format!("{key}: {} - this cannot be undone.", status_label(to, false)) }, label, "contract_transition", args),
            _ => f.run(label, "contract_transition", args),
        }
    }

    fn empty(&mut self, ui: &mut Ui, f: &mut Frame) {
        widgets::card(ui, |ui| {
            widgets::empty_state(
                ui,
                f.t("Noch keine Verträge", "No contracts yet"),
                f.t(
                    "Neuen Vertrag aus einer Vorlage erstellen oder Demo-Daten laden.",
                    "Create a contract from a template or load the demo data.",
                ),
            );
            ui.vertical_centered(|ui| {
                ui.horizontal(|ui| {
                    ui.add_space((ui.available_width() - 330.0).max(0.0) / 2.0);
                    if widgets::primary(ui, f.t("Neuer Vertrag", "New contract")).clicked() {
                        f.form("contract_draft", &[]);
                    }
                    if ui
                        .button(f.t("Demo-Daten laden", "Load demo data"))
                        .clicked()
                    {
                        f.run(
                            f.t("Demo-Daten", "Demo data"),
                            "contract_demo_seed",
                            json!({}),
                        );
                    }
                });
            });
        });
    }

    // ---------------- pipeline ----------------

    fn view_pipeline(&mut self, ui: &mut Ui, f: &mut Frame) {
        let de = f.lang_de;
        let active: i64 = self
            .rows
            .iter()
            .filter(|r| r.c.status == Status::Executed)
            .filter_map(|r| r.c.terms.annual_value_cents)
            .sum();
        let pending = self
            .rows
            .iter()
            .filter(|r| {
                matches!(
                    r.c.status,
                    Status::Draft | Status::UnderReview | Status::PendingSignature
                )
            })
            .count();
        let risky = self
            .rows
            .iter()
            .filter(|r| r.worst.as_deref() == Some("high") && r.c.status != Status::Cancelled)
            .count();
        let soon = self
            .rows
            .iter()
            .filter(|r| {
                r.deadline
                    .as_ref()
                    .and_then(|d| d["days_left"].as_i64())
                    .is_some_and(|d| d <= 60)
            })
            .count();
        widgets::page_header(ui, f.t("Vertrags-Pipeline", "Contract pipeline"), f.t("Karte ziehen ändert den Status (nur erlaubte Schritte; Unterzeichnen erst nach dem Versiegeln). Doppelklick öffnet den Vertrag.", "Drag a card to change its status (allowed steps only; executing needs a seal first). Double-click opens the contract."), |_| {});
        if self.rows.is_empty() {
            self.empty(ui, f);
            return;
        }
        widgets::kpi_row(
            ui,
            &[
                Kpi::new(f.t("Jahreswert aktiv", "Active annual value"), eur(active)),
                Kpi::new(f.t("In Arbeit", "In progress"), pending.to_string()),
                Kpi::new(f.t("Hohe Risiken", "High risks"), risky.to_string()).note(
                    if risky > 0 {
                        f.t("prüfen", "review")
                    } else {
                        "ok"
                    },
                    if risky > 0 { Tone::Danger } else { Tone::Ok },
                ),
                Kpi::new(
                    f.t("Fristen < 60 Tage", "Deadlines < 60 days"),
                    soon.to_string(),
                )
                .note(
                    if soon > 0 {
                        f.t("handeln", "act")
                    } else {
                        "ok"
                    },
                    if soon > 0 { Tone::Warn } else { Tone::Ok },
                ),
            ],
        );
        ui.add_space(6.0);
        let columns = vec![
            Column::new("draft", status_label(Status::Draft, de), Tone::Neutral),
            Column::new(
                "under_review",
                status_label(Status::UnderReview, de),
                Tone::Info,
            ),
            Column::new(
                "pending_signature",
                status_label(Status::PendingSignature, de),
                Tone::Warn,
            ),
            Column::new("executed", status_label(Status::Executed, de), Tone::Ok),
            Column::new("ended", f.t("Beendet", "Ended"), Tone::Violet),
        ];
        let cards: Vec<Card> = self
            .rows
            .iter()
            .filter(|r| app::search_hit(&f.search, &[&r.c.number, &r.c.title, &r.c.counterparty]))
            .map(|r| {
                let mut tags = Vec::new();
                if let Some(w) = &r.worst {
                    tags.push((
                        format!("{} {}", r.findings, f.t("Risiken", "risks")),
                        sev_tone(w),
                    ));
                }
                if r.sealed_current {
                    tags.push((f.t("versiegelt", "sealed").to_string(), Tone::Teal));
                }
                let mut meta = vec![
                    r.c.counterparty.clone(),
                    format!("v{}", r.c.current_version),
                ];
                if let Some(v) = r.c.terms.annual_value_cents {
                    meta.push(format!("{} / {}", eur(v), f.t("Jahr", "yr")));
                }
                let badge = r
                    .deadline
                    .as_ref()
                    .and_then(|d| d["days_left"].as_i64())
                    .map(|n| {
                        (
                            format!("{n} d"),
                            if n <= 30 {
                                Tone::Danger
                            } else if n <= 60 {
                                Tone::Warn
                            } else {
                                Tone::Neutral
                            },
                        )
                    });
                let ended = matches!(
                    r.c.status,
                    Status::Terminated | Status::Expired | Status::Cancelled
                );
                Card {
                    id: r.c.number.clone(),
                    column: column_of(r.c.status).into(),
                    lane: String::new(),
                    key: if ended {
                        format!("{}  {}", r.c.number, status_label(r.c.status, de))
                    } else {
                        r.c.number.clone()
                    },
                    title: r.c.title.clone(),
                    tags,
                    accent: status_tone(r.c.status),
                    badge,
                    assignee: None,
                    meta,
                    blocked: None,
                    done: ended,
                }
            })
            .collect();
        let refusals: Vec<(String, Refusals)> = self
            .rows
            .iter()
            .map(|r| {
                let per: Vec<(String, Option<String>)> = [
                    "draft",
                    "under_review",
                    "pending_signature",
                    "executed",
                    "ended",
                ]
                .iter()
                .map(|col| {
                    (
                        col.to_string(),
                        target_of(r.c.status, col).and_then(|to| self.refuse(r, to, de)),
                    )
                })
                .collect();
                (r.c.number.clone(), per)
            })
            .collect();
        let rule = move |c: &Card, to: &str| -> Result<(), String> {
            match refusals
                .iter()
                .find(|(k, _)| *k == c.id)
                .and_then(|(_, per)| per.iter().find(|(col, _)| col == to))
            {
                Some((_, Some(why))) => Err(why.clone()),
                _ => Ok(()),
            }
        };
        let events = {
            let r: kanban::Rule = &rule;
            Board::new("contract_pipeline", &columns, &cards)
                .rule(r)
                .add_buttons(false)
                .selected(self.page.as_ref().map(|p| p.key.as_str()))
                .show(ui)
        };
        for e in events {
            match e {
                kanban::Event::Moved { card, to, from, .. } if to != from => {
                    if let Some(st) = self.row(&card).and_then(|r| target_of(r.c.status, &to)) {
                        self.move_to(f, &card, st);
                    }
                }
                kanban::Event::Refused { reason, card, .. } => {
                    f.toast(format!("{card}: {reason}"), Tone::Warn)
                }
                kanban::Event::Open(k) => {
                    self.open(&k);
                }
                _ => {}
            }
        }
    }

    // ---------------- list ----------------

    fn view_list(&mut self, ui: &mut Ui, f: &mut Frame) {
        let de = f.lang_de;
        widgets::page_header(
            ui,
            f.t("Alle Verträge", "All contracts"),
            f.t("Klick öffnet den Vertrag.", "Click a contract to open it."),
            |ui| {
                if widgets::primary(ui, f.t("Neuer Vertrag", "New contract")).clicked() {
                    f.form("contract_draft", &[]);
                }
            },
        );
        if self.rows.is_empty() {
            self.empty(ui, f);
            return;
        }
        let cols = vec![
            Col::new(f.t("Nummer", "Number"), 1.0).min(96.0),
            Col::new(f.t("Titel", "Title"), 2.6),
            Col::new(f.t("Vertragspartner", "Counterparty"), 2.0),
            Col::new("Status", 1.3),
            Col::num(f.t("Jahreswert", "Annual value"), 1.0).prio(2),
            Col::new(f.t("Risiken", "Risks"), 1.0).prio(1),
            Col::new(f.t("Nächste Frist", "Next deadline"), 1.3).prio(3),
            Col::num("v", 0.4).prio(4),
        ];
        let data: Vec<(String, Vec<Cell>)> = self
            .rows
            .iter()
            .filter(|r| app::search_hit(&f.search, &[&r.c.number, &r.c.title, &r.c.counterparty]))
            .map(|r| {
                let dl = r.deadline.as_ref().map(|d| {
                    let n = d["days_left"].as_i64().unwrap_or(0);
                    (
                        format!(
                            "{} ({n} d)",
                            d["due"]
                                .as_str()
                                .and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok())
                                .map(date_de)
                                .unwrap_or_default()
                        ),
                        if n <= 30 {
                            Tone::Danger
                        } else if n <= 60 {
                            Tone::Warn
                        } else {
                            Tone::Neutral
                        },
                    )
                });
                (
                    r.c.number.clone(),
                    vec![
                        Cell::Mono(r.c.number.clone()),
                        Cell::Strong(r.c.title.clone()),
                        Cell::Text(r.c.counterparty.clone()),
                        Cell::Pill(status_label(r.c.status, de).into(), status_tone(r.c.status)),
                        money_cell(r.c.terms.annual_value_cents),
                        match &r.worst {
                            Some(w) => Cell::Pill(r.findings.to_string(), sev_tone(w)),
                            None => Cell::Weak("0".into()),
                        },
                        match dl {
                            Some((t, tone)) => Cell::Pill(t, tone),
                            None => Cell::Weak("-".into()),
                        },
                        Cell::Mono(r.c.current_version.to_string()),
                    ],
                )
            })
            .collect();
        let mut pick = None;
        widgets::card(ui, |ui| {
            for e in Table::new("contract_list", &cols, &data).show(ui) {
                let (TableEvent::Select(k) | TableEvent::Open(k)) = e;
                pick = Some(k);
            }
        });
        if let Some(k) = pick {
            self.open(&k);
        }
    }

    // ---------------- contract page ----------------

    fn view_page(&mut self, ui: &mut Ui, f: &mut Frame) {
        let de = f.lang_de;
        let Some(key) = self.page.as_ref().map(|p| p.key.clone()) else {
            return;
        };
        let Some(r) = self.row(&key) else {
            self.page = None;
            return;
        };
        let c = r.c.clone();
        let sealed = r.sealed_current;
        let deadline = r.deadline.clone();
        let tk = theme::current(ui.ctx());
        let mut back = false;
        ui.horizontal(|ui| {
            if ui.button(f.t("< Zurück", "< Back")).clicked() {
                back = true;
            }
            ui.label(
                RichText::new(&c.number)
                    .family(egui::FontFamily::Monospace)
                    .size(15.0),
            );
            ui.label(RichText::new(&c.title).family(semibold()).size(19.0));
            widgets::pill(ui, status_label(c.status, de), status_tone(c.status));
        });
        if back {
            self.page = None;
            return;
        }
        ui.add_space(4.0);
        // Status stepper.
        ui.horizontal_wrapped(|ui| {
            for (i, s) in [
                Status::Draft,
                Status::UnderReview,
                Status::PendingSignature,
                Status::Executed,
            ]
            .into_iter()
            .enumerate()
            {
                if i > 0 {
                    ui.label(RichText::new("\u{2192}").color(tk.text_weak));
                }
                let reached =
                    (c.status as u8) >= (s as u8) && !matches!(c.status, Status::Cancelled);
                let tone = if c.status == s {
                    status_tone(s)
                } else if reached {
                    Tone::Ok
                } else {
                    Tone::Neutral
                };
                widgets::pill(ui, status_label(s, de), tone);
            }
            ui.add_space(16.0);
            for to in c.status.next() {
                let why = self.row(&key).and_then(|r| self.refuse(r, to, de));
                let label = format!("{} {}", f.t("->", "->"), status_label(to, de));
                let btn = if matches!(to, Status::Cancelled | Status::Terminated | Status::Expired)
                {
                    egui::Button::new(
                        RichText::new(label).color(theme::tone_colors(Tone::Danger, tk.dark).0),
                    )
                } else {
                    egui::Button::new(RichText::new(label).family(semibold()))
                };
                let resp = ui.add_enabled(why.is_none(), btn);
                let resp = match &why {
                    Some(w) => resp.on_disabled_hover_text(w),
                    None => resp,
                };
                if resp.clicked() {
                    self.move_to(f, &key, to);
                }
            }
        });
        ui.add_space(6.0);
        // Right column: facts and actions.
        egui::Panel::right(("contract_facts", &key)).resizable(false).exact_size(300.0).frame(egui::Frame::new().fill(tk.surface).stroke(egui::Stroke::new(1.0, tk.border)).corner_radius(10).inner_margin(12)).show(ui, |ui| {
            egui::ScrollArea::vertical().id_salt("facts").show(ui, |ui| {
                widgets::section(ui, f.t("Vertragspartner", "Counterparty"), None);
                ui.horizontal(|ui| {
                    widgets::avatar(ui, &c.counterparty, 12.0);
                    ui.label(RichText::new(&c.counterparty).family(semibold()));
                });
                ui.add_space(8.0);
                widgets::section(ui, f.t("Bedingungen", "Terms"), None);
                let tm = &c.terms;
                widgets::field(ui, f.t("Beginn", "Start"), |ui| {
                    ui.label(tm.start.map(date_de).unwrap_or_else(|| "-".into()));
                });
                widgets::field(ui, f.t("Laufzeit", "Initial term"), |ui| {
                    ui.label(tm.initial_months.map(|m| format!("{m} {}", f.t("Monate", "months"))).unwrap_or_else(|| "-".into()));
                });
                widgets::field(ui, f.t("Verlängerung", "Renewal"), |ui| {
                    ui.label(if tm.renewal_months > 0 { format!("{} {}", tm.renewal_months, f.t("Monate", "months")) } else { f.t("fest", "fixed term").into() });
                });
                widgets::field(ui, f.t("Kündigungsfrist", "Notice"), |ui| {
                    ui.label(format!("{} {}", tm.notice_months, f.t("Monate", "months")));
                });
                widgets::field(ui, f.t("Jahreswert", "Annual value"), |ui| {
                    widgets::money(ui, &tm.annual_value_cents.map(eur).unwrap_or_else(|| "-".into()), false);
                });
                if let Some(d) = &deadline {
                    let n = d["days_left"].as_i64().unwrap_or(0);
                    let due = d["due"].as_str().and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()).map(date_de).unwrap_or_default();
                    let what = if d["kind"] == "notice" { f.t("Kündigen bis", "Notice by") } else { f.t("Endet am", "Ends on") };
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(format!("{what} {due}")).family(semibold()));
                        widgets::pill(ui, &format!("{n} {}", f.t("Tage", "days")), if n <= 30 { Tone::Danger } else if n <= 60 { Tone::Warn } else { Tone::Info });
                    });
                }
                let editable = !matches!(c.status, Status::Executed | Status::Terminated | Status::Expired | Status::Cancelled);
                let r = ui.add_enabled(editable, egui::Button::new(f.t("Bedingungen ändern...", "Edit terms...")));
                if r.on_disabled_hover_text(f.t("Nach der Unterschrift fix", "Fixed after execution")).clicked() {
                    let mut pre: Vec<(&str, String)> = vec![("contract", key.clone()), ("renewal_months", tm.renewal_months.to_string()), ("notice_months", tm.notice_months.to_string())];
                    if let Some(s) = tm.start {
                        pre.push(("start", date_de(s)));
                    }
                    if let Some(m) = tm.initial_months {
                        pre.push(("initial_months", m.to_string()));
                    }
                    if let Some(v) = tm.annual_value_cents {
                        pre.push(("annual_value", eur(v)));
                    }
                    let pre2: Vec<(&str, &str)> = pre.iter().map(|(k, v)| (*k, v.as_str())).collect();
                    f.form("contract_terms", &pre2);
                }
                ui.add_space(10.0);
                widgets::section(ui, f.t("Aktionen", "Actions"), None);
                if ui.add_enabled(!sealed, egui::Button::new(RichText::new(format!("{} v{}", f.t("Versiegeln", "Seal"), c.current_version)).family(semibold()))).on_disabled_hover_text(f.t("Diese Version ist versiegelt", "This version is sealed")).clicked() {
                    f.run_confirm(if de { format!("Version {} von {key} versiegeln (SHA-256 + Ed25519)?", c.current_version) } else { format!("Seal version {} of {key} (SHA-256 + Ed25519)?", c.current_version) }, f.t("Versiegelt", "Sealed"), "contract_seal", json!({ "contract": key }));
                }
                if ui.button(f.t("Siegel prüfen", "Verify seals")).clicked() {
                    f.run_show("contract_verify", json!({ "contract": key }));
                }
                if ui.button(f.t("Datei gegen Siegel prüfen...", "Verify a file...")).clicked() {
                    f.form("contract_verify", &[("contract", &key)]);
                }
                if ui.button(f.t("Exportieren (md, typ, pdf)...", "Export (md, typ, pdf)...")).clicked() {
                    f.form("contract_render", &[("contract", &key)]);
                }
                if ui.button(f.t("Ganzer Datensatz", "Full record")).clicked() {
                    f.run_show("contract_show", json!({ "contract": key }));
                }
                ui.label(RichText::new(f.t("Siegel = Manipulationsschutz, keine qualifizierte elektronische Signatur.", "A seal is tamper evidence, not a qualified electronic signature.")).color(tk.text_weak).size(12.5));
            });
        });
        // Tabs.
        let Some(p) = self.page.as_mut() else { return };
        ui.horizontal(|ui| {
            for (tab, label) in [
                (Tab::Text, format!("Text (v{})", c.current_version)),
                (
                    Tab::Versions,
                    format!(
                        "{} ({})",
                        if de { "Versionen" } else { "Versions" },
                        p.versions.len()
                    ),
                ),
                (
                    Tab::Risks,
                    format!(
                        "{} ({})",
                        if de { "Risiken" } else { "Risks" },
                        p.findings.len()
                    ),
                ),
                (
                    Tab::Seals,
                    format!(
                        "{} ({})",
                        if de { "Siegel" } else { "Seals" },
                        p.seals.len()
                    ),
                ),
                (
                    Tab::Terms,
                    (if de { "Fristen" } else { "Deadlines" }).to_string(),
                ),
                (
                    Tab::History,
                    format!(
                        "{} ({})",
                        if de { "Verlauf" } else { "History" },
                        p.history.len()
                    ),
                ),
            ] {
                if ui
                    .selectable_label(p.tab == tab, RichText::new(label).family(semibold()))
                    .clicked()
                {
                    p.tab = tab;
                }
            }
        });
        ui.separator();
        let draft = c.status == Status::Draft;
        egui::ScrollArea::vertical().id_salt(("page_tab", &key)).auto_shrink([false, false]).show(ui, |ui| match p.tab {
            Tab::Text => {
                if let Some((body, note)) = &mut p.edit {
                    let mut close = false;
                    ui.add(egui::TextEdit::multiline(body).code_editor().desired_rows(24).desired_width(f32::INFINITY));
                    ui.horizontal(|ui| {
                        ui.add(egui::TextEdit::singleline(note).hint_text(if de { "Was wurde geändert?" } else { "What changed?" }).desired_width(360.0));
                        let changed = body.trim() != p.body.trim();
                        if ui.add_enabled(changed, egui::Button::new(RichText::new(if de { "Als neue Version speichern" } else { "Save as new version" }).family(semibold()))).clicked() {
                            f.run(if de { "Neue Version" } else { "New version" }, "contract_edit", json!({ "contract": key, "body": body, "note": note }));
                            close = true;
                        } else if ui.button(if de { "Abbrechen" } else { "Cancel" }).clicked() {
                            close = true;
                        }
                    });
                    if close {
                        p.edit = None;
                    }
                } else {
                    let r = ui.add_enabled(draft, egui::Button::new(if de { "Text bearbeiten" } else { "Edit text" }));
                    if r.on_disabled_hover_text(if de { "Nur im Entwurf" } else { "Draft only" }).clicked() {
                        p.edit = Some((p.body.clone(), String::new()));
                    }
                    ui.add_space(6.0);
                    contract_text(ui, &p.body, &p.findings, de);
                }
            }
            Tab::Versions => {
                for v in p.versions.iter().rev() {
                    ui.horizontal(|ui| {
                        widgets::pill(ui, &format!("v{}", v.version), if v.version == c.current_version { Tone::Accent } else { Tone::Neutral });
                        ui.label(RichText::new(ts(&v.created_at)).family(egui::FontFamily::Monospace).size(13.0));
                        ui.label(if v.note.is_empty() { "-" } else { &v.note });
                        if p.seals.iter().any(|s| s.version == v.version) {
                            widgets::pill(ui, if de { "versiegelt" } else { "sealed" }, Tone::Teal);
                        }
                    });
                }
                ui.add_space(8.0);
                if c.current_version < 2 {
                    ui.label(if de { "Nur eine Version: nichts zu vergleichen." } else { "Only one version: nothing to compare." });
                } else {
                    ui.horizontal(|ui| {
                        ui.label(if de { "Vergleich" } else { "Compare" });
                        egui::ComboBox::from_id_salt("from").selected_text(format!("v{}", p.from)).show_ui(ui, |ui| {
                            for v in 1..=c.current_version {
                                ui.selectable_value(&mut p.from, v, format!("v{v}"));
                            }
                        });
                        ui.label("->");
                        egui::ComboBox::from_id_salt("to").selected_text(format!("v{}", p.to)).show_ui(ui, |ui| {
                            for v in 1..=c.current_version {
                                ui.selectable_value(&mut p.to, v, format!("v{v}"));
                            }
                        });
                        if ui.button(if de { "Redline-Bericht" } else { "Redline report" }).clicked() {
                            f.run_show("contract_redline", json!({ "contract": key, "from": p.from, "to": p.to }));
                        }
                    });
                    let a = p.versions.iter().find(|v| v.version == p.from).map(|v| v.body.clone()).unwrap_or_default();
                    let b = p.versions.iter().find(|v| v.version == p.to).map(|v| v.body.clone()).unwrap_or_default();
                    redline(ui, &a, &b);
                }
            }
            Tab::Risks => {
                ui.horizontal(|ui| {
                    ui.label(if de { "Regelbasierte Prüfung (TOML-Regeln, keine KI)." } else { "Rule-based check (TOML rules, not AI)." });
                    if ui.button(if de { "Voller Scan" } else { "Full scan" }).clicked() {
                        f.run_show("contract_risk_scan", json!({ "contract": key }));
                    }
                });
                if p.findings.is_empty() {
                    widgets::empty_state(ui, if de { "Keine Treffer" } else { "No findings" }, if de { "Keine Regel schlägt an." } else { "No rule matched." });
                }
                findings_list(ui, &p.findings, de);
            }
            Tab::Seals => {
                if p.seals.is_empty() {
                    widgets::empty_state(ui, if de { "Noch nicht versiegelt" } else { "Not sealed yet" }, if de { "Vor dem Unterzeichnen die aktuelle Version versiegeln." } else { "Seal the current version before executing." });
                }
                for s in p.seals.iter().rev() {
                    widgets::card(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            widgets::pill(ui, &format!("v{}", s.version), Tone::Teal);
                            ui.label(RichText::new(ts(&s.sealed_at)).family(egui::FontFamily::Monospace));
                        });
                        ui.label(RichText::new(format!("SHA-256 {}", s.sha256)).family(egui::FontFamily::Monospace).size(12.5));
                        ui.label(RichText::new(format!("Ed25519 {}...", s.public_key.chars().take(24).collect::<String>())).family(egui::FontFamily::Monospace).size(12.5).color(tk.text_weak));
                    });
                    ui.add_space(4.0);
                }
            }
            Tab::Terms => {
                match &deadline {
                    Some(d) => {
                        let n = d["days_left"].as_i64().unwrap_or(0);
                        widgets::kpi_row(ui, &[Kpi::new(if d["kind"] == "notice" { if de { "Kündigen bis" } else { "Notice by" } } else if de { "Endet am" } else { "Ends on" }, d["due"].as_str().and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()).map(date_de).unwrap_or_default()).note(format!("{n} {}", if de { "Tage" } else { "days" }), if n <= 30 { Tone::Danger } else if n <= 60 { Tone::Warn } else { Tone::Info }), Kpi::new(if de { "Laufzeitende" } else { "Term end" }, d["term_end"].as_str().and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()).map(date_de).unwrap_or_default())]);
                    }
                    None => {
                        ui.label(if de { "Keine Frist: Beginn und Laufzeit fehlen oder der Vertrag ist abgelaufen." } else { "No deadline: start and term are missing or the contract has ended." });
                    }
                }
                ui.label(RichText::new(if de { "Fristberechnung mit Monatsende-Regel (craft-biz deadline). Fristen gehen über Fristen > An taskcraft senden als Aufgaben weiter." } else { "Deadlines use the month-end rule (craft-biz deadline). Deadlines > Send to taskcraft turns them into issues." }).color(tk.text_weak));
            }
            Tab::History => {
                for h in p.history.iter().rev() {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new(ts(&h.at)).family(egui::FontFamily::Monospace).size(13.0));
                        widgets::avatar(ui, &h.actor, 9.0);
                        ui.label(&h.actor);
                        ui.label(RichText::new(format!("{} -> {}", h.from, h.to)).family(semibold()));
                        if !h.note.is_empty() {
                            ui.label(RichText::new(&h.note).color(tk.text_weak));
                        }
                    });
                }
            }
        });
    }

    // ---------------- other views ----------------

    fn view_deadlines(&mut self, ui: &mut Ui, f: &mut Frame) {
        widgets::page_header(
            ui,
            f.t("Fristen", "Deadlines"),
            f.t(
                "Nächste Kündigungsfrist oder Ablauf pro Vertrag, nächste 12 Monate.",
                "Next notice date or expiry per contract, next 12 months.",
            ),
            |ui| {
                if widgets::primary(
                    ui,
                    f.t(
                        "An taskcraft senden (90 Tage)",
                        "Send to taskcraft (90 days)",
                    ),
                )
                .on_hover_text("contract.deadline events, once per deadline")
                .clicked()
                {
                    f.run_show(
                        "contract_deadlines",
                        json!({ "within_days": 90, "emit": true }),
                    );
                }
                if ui.button(f.t("Bericht", "Report")).clicked() {
                    f.run_show("contract_deadlines", json!({ "within_days": 365 }));
                }
            },
        );
        let mut list: Vec<&Row> = self
            .rows
            .iter()
            .filter(|r| {
                r.deadline
                    .as_ref()
                    .and_then(|d| d["days_left"].as_i64())
                    .is_some_and(|n| n <= 365)
            })
            .collect();
        list.sort_by_key(|r| {
            r.deadline
                .as_ref()
                .and_then(|d| d["days_left"].as_i64())
                .unwrap_or(9999)
        });
        if list.is_empty() {
            widgets::card(ui, |ui| {
                widgets::empty_state(ui, f.t("Keine Fristen", "No deadlines"), f.t("Bedingungen (Beginn, Laufzeit, Kündigungsfrist) eintragen, dann erscheinen Fristen hier.", "Enter terms (start, term, notice) and deadlines appear here."))
            });
            return;
        }
        let cols = vec![
            Col::num(f.t("Tage", "Days"), 0.6),
            Col::new(f.t("Datum", "Date"), 1.0),
            Col::new(f.t("Art", "Kind"), 1.0),
            Col::new(f.t("Vertrag", "Contract"), 1.0),
            Col::new(f.t("Titel", "Title"), 2.4),
            Col::new(f.t("Partner", "Counterparty"), 1.8).prio(1),
            Col::num(f.t("Jahreswert", "Annual value"), 1.0).prio(2),
        ];
        let data: Vec<(String, Vec<Cell>)> = list
            .iter()
            .filter_map(|r| {
                let d = r.deadline.as_ref()?;
                let n = d["days_left"].as_i64().unwrap_or(0);
                let notice = d["kind"] == "notice";
                Some((
                    r.c.number.clone(),
                    vec![
                        Cell::Pill(
                            n.to_string(),
                            if n <= 30 {
                                Tone::Danger
                            } else if n <= 60 {
                                Tone::Warn
                            } else {
                                Tone::Info
                            },
                        ),
                        Cell::Mono(
                            d["due"]
                                .as_str()
                                .and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok())
                                .map(date_de)
                                .unwrap_or_default(),
                        ),
                        Cell::Pill(
                            if notice {
                                f.t("Kündigungsfrist", "Notice").into()
                            } else {
                                f.t("Ablauf", "Expiry").into()
                            },
                            if notice { Tone::Warn } else { Tone::Info },
                        ),
                        Cell::Mono(r.c.number.clone()),
                        Cell::Strong(r.c.title.clone()),
                        Cell::Text(r.c.counterparty.clone()),
                        money_cell(r.c.terms.annual_value_cents),
                    ],
                ))
            })
            .collect();
        let mut pick = None;
        widgets::card(ui, |ui| {
            for e in Table::new("contract_deadlines", &cols, &data)
                .filter_box(false)
                .show(ui)
            {
                let (TableEvent::Select(k) | TableEvent::Open(k)) = e;
                pick = Some(k);
            }
        });
        if let Some(k) = pick {
            self.open(&k);
            f.goto(V_LIST);
        }
    }

    fn view_templates(&mut self, ui: &mut Ui, f: &mut Frame) {
        widgets::page_header(ui, f.t("Vorlagen", "Templates"), f.t("Versionierte Vorlagen mit Platzhaltern {{name}}. Ein Vertrag merkt sich die Vorlagenversion.", "Versioned templates with {{name}} placeholders. A contract remembers its template version."), |ui| {
            if widgets::primary(ui, f.t("Neue Vorlage", "New template")).clicked() {
                f.form("contract_template_add", &[]);
            }
            if ui.button(f.t("Als Liste", "As list")).clicked() {
                f.run_show("contract_template_list", json!({}));
            }
        });
        if self.templates.is_empty() {
            widgets::card(ui, |ui| {
                widgets::empty_state(
                    ui,
                    f.t("Keine Vorlagen", "No templates"),
                    f.t(
                        "Neue Vorlage anlegen oder Demo-Daten laden.",
                        "Add a template or load the demo data.",
                    ),
                )
            });
        }
        let tk = theme::current(ui.ctx());
        for t in &self.templates {
            widgets::card(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(RichText::new(&t.name).family(semibold()).size(16.0));
                    widgets::pill(ui, &format!("v{}", t.version), Tone::Neutral);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if widgets::primary(ui, f.t("Vertrag daraus", "Use template")).clicked() {
                            let vals: serde_json::Map<String, Value> = t
                                .variables
                                .iter()
                                .map(|v| {
                                    (v.name.clone(), json!(v.default.clone().unwrap_or_default()))
                                })
                                .collect();
                            let vals = Value::Object(vals).to_string();
                            f.form(
                                "contract_draft",
                                &[("template", &t.name), ("values", &vals)],
                            );
                        }
                        if ui.button(f.t("Anzeigen", "Show")).clicked() {
                            f.run_show("contract_template_show", json!({ "name": t.name }));
                        }
                    });
                });
                if !t.description.is_empty() {
                    ui.label(&t.description);
                }
                ui.horizontal_wrapped(|ui| {
                    for v in &t.variables {
                        widgets::pill(
                            ui,
                            &format!("{}{}", v.name, if v.required { " *" } else { "" }),
                            if v.required {
                                Tone::Accent
                            } else {
                                Tone::Neutral
                            },
                        )
                        .on_hover_text(&v.description);
                    }
                });
                ui.label(
                    RichText::new(t.body.chars().take(240).collect::<String>())
                        .color(tk.text_weak)
                        .size(13.0),
                );
            });
            ui.add_space(6.0);
        }
    }

    fn view_risk(&mut self, ui: &mut Ui, f: &mut Frame) {
        let de = f.lang_de;
        widgets::page_header(ui, f.t("Risiko-Check", "Risk check"), f.t("Fremden Vertragstext einfügen und gegen die Regeln prüfen. Nichts wird gespeichert.", "Paste a contract you received and check it against the rules. Nothing is stored."), |_| {});
        widgets::card(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.add(
                egui::TextEdit::multiline(&mut self.risk_text)
                    .desired_rows(14)
                    .desired_width(f32::INFINITY)
                    .hint_text(f.t(
                        "Vertragstext hier einfügen...",
                        "Paste contract text here...",
                    )),
            );
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        !self.risk_text.trim().is_empty(),
                        egui::Button::new(RichText::new(f.t("Prüfen", "Check")).family(semibold())),
                    )
                    .clicked()
                {
                    self.risk_out = Some(risk::scan(&self.rules, &self.risk_text));
                }
                if ui
                    .add_enabled(
                        !self.risk_text.trim().is_empty(),
                        egui::Button::new(f.t("Bericht (Tool)", "Report (tool)")),
                    )
                    .clicked()
                {
                    f.run_show("contract_risk_scan", json!({ "text": self.risk_text }));
                }
            });
        });
        if let Some(out) = &self.risk_out {
            ui.add_space(8.0);
            if out.is_empty() {
                widgets::card(ui, |ui| {
                    widgets::empty_state(
                        ui,
                        f.t("Keine Treffer", "No findings"),
                        f.t(
                            "Keine Regel schlägt an. Das ersetzt keine Rechtsberatung.",
                            "No rule matched. This is no substitute for legal advice.",
                        ),
                    )
                });
            } else {
                findings_list(ui, out, de);
            }
        }
    }

    fn view_settings(&mut self, ui: &mut Ui, f: &mut Frame) {
        widgets::page_header(ui, f.t("Einstellungen", "Settings"), &self.db, |_| {});
        widgets::card(ui, |ui| {
            ui.set_width(ui.available_width());
            widgets::field(ui, f.t("Siegelschlüssel", "Seal key"), |ui| {
                ui.label(
                    RichText::new(self.ctx.key_path.display().to_string())
                        .family(egui::FontFamily::Monospace),
                );
            });
            widgets::field(ui, f.t("Risikoregeln", "Risk rules"), |ui| {
                ui.label(format!(
                    "{} {}",
                    self.rules.len(),
                    f.t("Standardregeln", "default rules")
                ));
            });
            ui.horizontal(|ui| {
                if ui.button("Status").clicked() {
                    f.run_show("contract_status", json!({}));
                }
                if ui
                    .button(f.t("Neueinstellung als Entwurf", "Take new hires"))
                    .clicked()
                {
                    f.run(
                        f.t("Neueinstellung", "New hire"),
                        "contract_drain_inbox",
                        json!({}),
                    );
                }
                if ui
                    .add_enabled(
                        self.rows.is_empty(),
                        egui::Button::new(f.t("Demo-Daten laden", "Load demo data")),
                    )
                    .on_disabled_hover_text("only on an empty database")
                    .clicked()
                {
                    f.run(
                        f.t("Demo-Daten", "Demo data"),
                        "contract_demo_seed",
                        json!({}),
                    );
                }
            });
        });
    }
}

/// Contract text by paragraph, numbered; paragraphs with findings get a coloured bar and the rule titles.
fn contract_text(ui: &mut Ui, body: &str, findings: &[risk::Finding], de: bool) {
    let tk = theme::current(ui.ctx());
    for (i, para) in body.split("\n\n").enumerate() {
        let n = i + 1;
        let hits: Vec<&risk::Finding> = findings.iter().filter(|x| x.paragraph == n).collect();
        let worst = hits
            .iter()
            .map(|h| h.severity.as_str())
            .max_by_key(|s| sev_rank(s));
        let fill = worst
            .map(|w| theme::tone_colors(sev_tone(w), tk.dark).1)
            .unwrap_or(tk.surface);
        egui::Frame::new()
            .fill(fill)
            .corner_radius(6)
            .inner_margin(egui::Margin::symmetric(10, 6))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal_top(|ui| {
                    ui.add_sized(
                        [28.0, 18.0],
                        egui::Label::new(
                            RichText::new(n.to_string())
                                .family(egui::FontFamily::Monospace)
                                .color(tk.text_weak),
                        ),
                    );
                    ui.vertical(|ui| {
                        ui.add(egui::Label::new(RichText::new(para.trim()).size(14.5)).wrap());
                        for h in &hits {
                            ui.horizontal_wrapped(|ui| {
                                widgets::pill(ui, &h.severity, sev_tone(&h.severity));
                                ui.label(RichText::new(&h.title).family(semibold()));
                                ui.label(RichText::new(&h.advice).size(13.0));
                            });
                        }
                    });
                });
            });
        ui.add_space(2.0);
    }
    if body.trim().is_empty() {
        ui.label(if de { "(leerer Text)" } else { "(empty text)" });
    }
}

fn findings_list(ui: &mut Ui, findings: &[risk::Finding], de: bool) {
    let mut sorted: Vec<&risk::Finding> = findings.iter().collect();
    sorted.sort_by_key(|x| (std::cmp::Reverse(sev_rank(&x.severity)), x.paragraph));
    for x in sorted {
        widgets::card(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                widgets::pill(ui, &x.severity, sev_tone(&x.severity));
                ui.label(RichText::new(&x.title).family(semibold()).size(15.0));
                ui.label(
                    RichText::new(format!(
                        "{} {}",
                        if de { "Absatz" } else { "paragraph" },
                        x.paragraph
                    ))
                    .color(theme::current(ui.ctx()).text_weak),
                );
            });
            ui.label(RichText::new(format!("\u{201C}{}\u{201D}", x.excerpt)).italics());
            ui.label(&x.advice);
        });
        ui.add_space(4.0);
    }
}

/// Inline redline: removed lines struck through in red, added lines in green, unchanged lines plain.
fn redline(ui: &mut Ui, a: &str, b: &str) {
    let tk = theme::current(ui.ctx());
    let diff = TextDiff::from_lines(a, b);
    let (dfg, dbg) = theme::tone_colors(Tone::Danger, tk.dark);
    let (ifg, ibg) = theme::tone_colors(Tone::Ok, tk.dark);
    egui::Frame::new()
        .fill(tk.surface)
        .stroke(egui::Stroke::new(1.0, tk.border))
        .corner_radius(8)
        .inner_margin(10)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            for ch in diff.iter_all_changes() {
                let line = ch.value().trim_end_matches('\n');
                match ch.tag() {
                    ChangeTag::Equal => {
                        ui.label(
                            RichText::new(format!("  {line}"))
                                .family(egui::FontFamily::Monospace)
                                .size(13.0),
                        );
                    }
                    ChangeTag::Delete => {
                        egui::Frame::new().fill(dbg).show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.label(
                                RichText::new(format!("- {line}"))
                                    .family(egui::FontFamily::Monospace)
                                    .size(13.0)
                                    .color(dfg)
                                    .strikethrough(),
                            );
                        });
                    }
                    ChangeTag::Insert => {
                        egui::Frame::new().fill(ibg).show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.label(
                                RichText::new(format!("+ {line}"))
                                    .family(egui::FontFamily::Monospace)
                                    .size(13.0)
                                    .color(ifg),
                            );
                        });
                    }
                }
            }
        });
}

impl SuiteApp for ContractApp {
    fn name(&self) -> &'static str {
        "ContractCraft"
    }
    fn id(&self) -> &'static str {
        contractcraft_core::APP
    }
    fn tone(&self) -> Tone {
        Tone::Info
    }
    fn version(&self) -> &'static str {
        contractcraft_core::VERSION
    }
    fn tools(&self) -> Vec<Value> {
        tools()
    }
    fn call(&mut self, tool: &str, args: &Value) -> Result<Value, String> {
        let cmd = Command::from_tool(tool, args)?;
        dispatch(&mut self.ctx, cmd)
    }
    fn reload(&mut self) -> Result<(), String> {
        self.today = Local::now().date_naive();
        let db = &self.ctx.db;
        let mut rows = Vec::new();
        for c in db.contracts(None, None).map_err(|e| e.to_string())? {
            let seals = db.seals(&c).map_err(|e| e.to_string())?;
            let body = db.version(&c, None).map_err(|e| e.to_string())?.body;
            let findings = risk::scan(&self.rules, &body);
            let worst = findings
                .iter()
                .map(|x| x.severity.clone())
                .max_by_key(|s| sev_rank(s));
            let deadline = if matches!(
                c.status,
                Status::Cancelled | Status::Terminated | Status::Expired
            ) {
                None
            } else {
                next_deadline(&c, self.today)?
            };
            rows.push(Row {
                sealed_current: seals.iter().any(|s| s.version == c.current_version),
                deadline,
                findings: findings.len(),
                worst,
                c,
            });
        }
        self.rows = rows;
        self.templates = db.templates().map_err(|e| e.to_string())?;
        if let Some(k) = self.page.as_ref().map(|p| p.key.clone()) {
            self.open(&k);
        }
        Ok(())
    }
    fn nav(&self, f: &Frame) -> Vec<NavEntry> {
        let open = self
            .rows
            .iter()
            .filter(|r| {
                matches!(
                    r.c.status,
                    Status::Draft | Status::UnderReview | Status::PendingSignature
                )
            })
            .count();
        let soon = self
            .rows
            .iter()
            .filter(|r| {
                r.deadline
                    .as_ref()
                    .and_then(|d| d["days_left"].as_i64())
                    .is_some_and(|d| d <= 60)
            })
            .count();
        vec![
            NavEntry::new("\u{25A6}", f.t("Pipeline", "Pipeline"))
                .group(f.t("Verträge", "Contracts"))
                .badge(open, Tone::Info),
            NavEntry::new("\u{2630}", f.t("Alle Verträge", "All contracts")),
            NavEntry::new("\u{23F1}", f.t("Fristen", "Deadlines")).badge(soon, Tone::Warn),
            NavEntry::new("\u{2750}", f.t("Vorlagen", "Templates"))
                .group(f.t("Werkzeuge", "Tools")),
            NavEntry::new("\u{26A0}", f.t("Risiko-Check", "Risk check")),
            NavEntry::new("\u{2699}", f.t("Einstellungen", "Settings")),
        ]
    }
    fn view(&mut self, ui: &mut Ui, view: usize, f: &mut Frame) {
        let leave = match self.page.as_mut() {
            Some(p) if p.view.is_none() => {
                p.view = Some(view);
                false
            }
            Some(p) => p.view != Some(view),
            None => false,
        };
        if leave {
            self.page = None;
        }
        if self.page.is_some() && view <= V_DEADLINES {
            self.view_page(ui, f);
            return;
        }
        egui::ScrollArea::vertical()
            .id_salt(("contract_view", view))
            .auto_shrink([false, false])
            .show(ui, |ui| match view {
                V_PIPELINE => self.view_pipeline(ui, f),
                V_LIST => self.view_list(ui, f),
                V_DEADLINES => self.view_deadlines(ui, f),
                V_TEMPLATES => self.view_templates(ui, f),
                V_RISK => self.view_risk(ui, f),
                V_SETTINGS => self.view_settings(ui, f),
                _ => self.view_pipeline(ui, f),
            });
    }
    fn top_actions(&mut self, ui: &mut Ui, f: &mut Frame) {
        if widgets::primary(ui, f.t("+ Vertrag", "+ Contract")).clicked() {
            f.form("contract_draft", &[]);
        }
    }
    fn bus(&self) -> &Bus {
        &self.ctx.bus
    }
    fn wiring(&self) -> Wiring {
        (
            vec![],
            vec![
                (
                    "contract.executed",
                    "to gaincraft: ARR, contract number, notice date",
                ),
                (
                    "contract.deadline",
                    "to taskcraft: a notice date becomes an issue",
                ),
            ],
        )
    }
    fn prefill(&self) -> Vec<(String, String)> {
        self.page
            .iter()
            .map(|p| ("contract".to_string(), p.key.clone()))
            .collect()
    }
    fn select(&mut self, key: &str) {
        self.open(key);
    }
    fn status_info(&self) -> String {
        format!("{}  |  {} contracts", self.db, self.rows.len())
    }
    fn smoke_tool(&self) -> &'static str {
        "contract_status"
    }
}

fn default_db() -> PathBuf {
    if let Ok(p) = std::env::var("CONTRACTCRAFT_DB") {
        if !p.trim().is_empty() {
            return PathBuf::from(p);
        }
    }
    std::env::var("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("contractcraft")
        .join("contractcraft.db")
}

fn open(db: &Path) -> Result<ContractApp, String> {
    let store = Contracts::open(db).map_err(|e| e.to_string())?;
    let actor = std::env::var("USERNAME")
        .or_else(|_| std::env::var("USER"))
        .unwrap_or_else(|_| "desktop".into());
    let key_path = db
        .parent()
        .map(|p| p.join("seal.key"))
        .unwrap_or_else(|| PathBuf::from("seal.key"));
    let ctx = Ctx {
        db: store,
        confinement: Confinement(None),
        bus: Bus::new(Bus::default_root()),
        key_path,
        actor,
    };
    Ok(ContractApp::new(ctx, db.display().to_string()))
}

fn main() -> std::process::ExitCode {
    app::main_with(default_db(), open, "contract_demo_seed")
}

#[cfg(test)]
mod tests {
    use super::*;
    use craft_biz_ui::app::Shell;
    use egui_kittest::Harness;

    fn shell(tag: &str) -> (Shell<ContractApp>, PathBuf) {
        let dir =
            std::env::temp_dir().join(format!("contractcraft-app-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let ctx = Ctx {
            db: Contracts::open_in_memory().unwrap(),
            confinement: Confinement(None),
            bus: Bus::new(dir.join("bus")),
            key_path: dir.join("seal.key"),
            actor: "Anna Mockova".into(),
        };
        let mut app = ContractApp::new(ctx, ":memory:".into());
        app.call("contract_demo_seed", &json!({})).unwrap();
        (Shell::new(app, None), dir)
    }

    #[test]
    fn every_tool_has_a_place_in_the_window() {
        craft_biz_ui::coverage::check(&tools(), COVERAGE).unwrap();
    }

    #[test]
    fn every_view_and_every_page_tab_render() {
        let (sh, dir) = shell("render");
        let key = sh
            .app
            .rows
            .iter()
            .find(|r| r.c.current_version > 1)
            .or(sh.app.rows.first())
            .map(|r| r.c.number.clone())
            .unwrap();
        let mut h = Harness::builder()
            .with_size([1500.0, 900.0])
            .build_ui_state(|ui, sh: &mut Shell<ContractApp>| sh.show(ui), sh);
        for v in 0..6 {
            h.state_mut().view = v;
            h.run_steps(3);
        }
        h.state_mut().view = V_LIST;
        h.run_steps(2);
        h.state_mut().app.open(&key);
        for tab in [
            Tab::Text,
            Tab::Versions,
            Tab::Risks,
            Tab::Seals,
            Tab::Terms,
            Tab::History,
        ] {
            if let Some(p) = h.state_mut().app.page.as_mut() {
                p.tab = tab;
            }
            h.run_steps(2);
        }
        craft_biz_ui::theme::apply(&h.ctx, true);
        h.run_steps(2);
        assert!(h.state().last_error.is_none(), "{:?}", h.state().last_error);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn execute_needs_a_seal_and_the_board_says_why() {
        let (mut sh, dir) = shell("seal");
        let r = sh
            .app
            .rows
            .iter()
            .find(|r| r.c.status == Status::PendingSignature || r.c.status == Status::UnderReview)
            .map(|r| r.c.number.clone());
        let key = match r {
            Some(k) => k,
            None => return,
        };
        if sh.app.row(&key).map(|r| r.c.status) == Some(Status::UnderReview) {
            sh.run_tool(
                "contract_transition",
                json!({ "contract": key, "to": "pending_signature" }),
            );
        }
        let row = sh.app.row(&key).unwrap();
        assert!(!row.sealed_current);
        let why = sh.app.refuse(row, Status::Executed, false).unwrap();
        assert!(why.contains("Seal version"), "{why}");
        assert!(sh.app.refuse(row, Status::Draft, false).is_none());
        sh.run_tool("contract_seal", json!({ "contract": key }));
        let row = sh.app.row(&key).unwrap();
        assert!(row.sealed_current, "{:?}", sh.last_error);
        assert!(sh.app.refuse(row, Status::Executed, false).is_none());
        sh.run_tool(
            "contract_transition",
            json!({ "contract": key, "to": "executed" }),
        );
        assert_eq!(sh.app.row(&key).map(|r| r.c.status), Some(Status::Executed));
        assert!(
            sh.app
                .bus()
                .outgoing("contractcraft", 10)
                .unwrap()
                .iter()
                .any(|(e, _)| e.kind == "contract.executed"),
            "gaincraft event sent"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn board_refuses_a_skip_and_moves_an_allowed_step() {
        let (mut sh, dir) = shell("board");
        sh.view = V_PIPELINE;
        let key = sh
            .app
            .rows
            .iter()
            .find(|r| r.c.status == Status::Draft)
            .map(|r| r.c.number.clone())
            .unwrap();
        let mut h = Harness::builder()
            .with_size([1700.0, 950.0])
            .build_ui_state(|ui, sh: &mut Shell<ContractApp>| sh.show(ui), sh);
        h.run_steps(3);
        let drag = |h: &mut Harness<'_, Shell<ContractApp>>, col: &str| {
            let card = kanban::card_rect(&h.ctx, "contract_pipeline", &key).unwrap();
            let target = kanban::cell_rect(&h.ctx, "contract_pipeline", col, "").unwrap();
            h.hover_at(card.center());
            h.run_steps(1);
            h.drag_at(card.center());
            h.run_steps(1);
            for i in 1..=8 {
                h.hover_at(card.center() + (target.center() - card.center()) * (i as f32 / 8.0));
                h.run_steps(1);
            }
            h.drop_at(target.center());
            h.run_steps(3);
        };
        drag(&mut h, "executed");
        assert_eq!(
            h.state().app.row(&key).map(|r| r.c.status),
            Some(Status::Draft),
            "draft -> executed refused"
        );
        drag(&mut h, "under_review");
        assert_eq!(
            h.state().app.row(&key).map(|r| r.c.status),
            Some(Status::UnderReview),
            "{:?}",
            h.state().last_error
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn redline_and_paragraph_findings_are_computed_locally() {
        let rules = risk::default_rules().unwrap();
        let text = "Der Vertrag verlängert sich automatisch.\n\nHaftung ist unbeschränkt.";
        let f = risk::scan(&rules, text);
        assert!(f.iter().all(|x| x.paragraph >= 1 && x.paragraph <= 2));
        let d = TextDiff::from_lines("a\nb\n", "a\nc\n");
        let ops: Vec<ChangeTag> = d.iter_all_changes().map(|c| c.tag()).collect();
        assert_eq!(
            ops,
            vec![ChangeTag::Equal, ChangeTag::Delete, ChangeTag::Insert]
        );
    }
}
