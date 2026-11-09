//! contractcraft-core: the command bus shared by CLI, MCP and the desktop app.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

pub mod render;
pub mod risk;
pub mod seal;

use chrono::{Local, Months, NaiveDate, Utc};
use contractcraft_store::{
    fill, Contract, ContractError, Contracts, NewContract, Status, Terms, Variable,
};
use craft_biz::at::parse_date;
use craft_biz::bus::Bus;
pub use craft_biz::confine::Confinement;
use craft_biz::deadline::AutoRenewal;
use craft_biz::mcp::tool;
use craft_biz::Money;
use serde_json::{json, Value};
use similar::{ChangeTag, TextDiff};
use std::collections::BTreeMap;
use std::path::PathBuf;

pub const APP: &str = "contractcraft";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;

pub struct Ctx {
    pub db: Contracts,
    pub confinement: Confinement,
    pub bus: Bus,
    /// Ed25519 seal key file (created on first seal).
    pub key_path: PathBuf,
    pub actor: String,
}

#[derive(Debug, Clone)]
pub enum Command {
    Status,
    TemplateAdd {
        name: String,
        description: String,
        variables: Vec<Variable>,
        body: Source,
    },
    TemplateList,
    TemplateShow {
        name: String,
        version: Option<i64>,
    },
    Draft(DraftReq),
    List {
        status: Option<Status>,
        text: Option<String>,
    },
    Show {
        contract: String,
        version: Option<i64>,
        as_of: Option<NaiveDate>,
    },
    Edit {
        contract: String,
        body: Source,
        note: String,
    },
    Terms {
        contract: String,
        terms: Terms,
    },
    Transition {
        contract: String,
        to: Status,
        note: String,
    },
    Seal {
        contract: String,
    },
    Verify {
        contract: String,
        path: Option<String>,
    },
    Redline {
        contract: String,
        from: Option<i64>,
        to: Option<i64>,
    },
    RiskScan {
        contract: Option<String>,
        text: Option<String>,
        version: Option<i64>,
        rules_path: Option<String>,
    },
    Render {
        contract: String,
        format: String,
        path: String,
        version: Option<i64>,
    },
    Deadlines {
        within_days: i64,
        as_of: Option<NaiveDate>,
        emit: bool,
    },
    DemoSeed,
}

#[derive(Debug, Clone)]
pub struct DraftReq {
    pub template: Option<String>,
    pub title: String,
    pub counterparty: String,
    pub values: BTreeMap<String, String>,
    pub body: Option<Source>,
}

/// Text given inline or as a file path.
#[derive(Debug, Clone)]
pub enum Source {
    Text(String),
    File(String),
}

// ---------- argument helpers ----------

fn s<'a>(a: &'a Value, k: &str) -> Result<&'a str, String> {
    a.get(k)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("missing string argument {k:?}"))
}
fn opt_s(a: &Value, k: &str) -> Option<String> {
    a.get(k)
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|v| !v.trim().is_empty())
}
fn e(err: ContractError) -> String {
    err.to_string()
}
fn date(v: &str) -> Result<NaiveDate, String> {
    parse_date(v).map_err(|x| x.to_string())
}
fn opt_date(a: &Value, k: &str) -> Result<Option<NaiveDate>, String> {
    opt_s(a, k).map(|d| date(&d)).transpose()
}
fn int(a: &Value, k: &str) -> Result<Option<i64>, String> {
    match a.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_i64()
            .map(Some)
            .ok_or_else(|| format!("{k} must be an integer")),
    }
}
fn flag(a: &Value, k: &str) -> bool {
    a.get(k).and_then(Value::as_bool).unwrap_or(false)
}
fn today() -> NaiveDate {
    Local::now().date_naive()
}
fn eur(c: i64) -> String {
    Money::from_cents(c).format_de()
}
fn source(a: &Value, text_key: &str, path_key: &str) -> Result<Option<Source>, String> {
    match (a.get(text_key).and_then(Value::as_str), opt_s(a, path_key)) {
        (Some(_), Some(_)) => Err(format!("pass {text_key} or {path_key}, not both")),
        (Some(t), None) => Ok(Some(Source::Text(t.to_string()))),
        (None, Some(p)) => Ok(Some(Source::File(p))),
        (None, None) => Ok(None),
    }
}
fn values(a: &Value) -> Result<BTreeMap<String, String>, String> {
    match a.get("values") {
        None | Some(Value::Null) => Ok(BTreeMap::new()),
        Some(Value::Object(m)) => m
            .iter()
            .map(|(k, v)| match v {
                Value::String(s) => Ok((k.clone(), s.clone())),
                Value::Number(n) => Ok((k.clone(), n.to_string())),
                _ => Err(format!("value {k} must be a string")),
            })
            .collect(),
        _ => Err("values must be an object of strings".into()),
    }
}

impl Command {
    pub fn from_tool(name: &str, a: &Value) -> Result<Command, String> {
        let name = craft_biz::mcp::normalize_tool_name(name);
        Ok(match name.as_str() {
            "contract_status" => Command::Status,
            "contract_template_add" => Command::TemplateAdd {
                name: s(a, "name")?.into(),
                description: opt_s(a, "description").unwrap_or_default(),
                variables: match a.get("variables") {
                    None | Some(Value::Null) => Vec::new(),
                    Some(v) => serde_json::from_value(v.clone()).map_err(|x| {
                        format!("variables: {x} (list of {{name, required, default, description}})")
                    })?,
                },
                body: source(a, "body", "body_path")?.ok_or("pass body or body_path")?,
            },
            "contract_template_list" => Command::TemplateList,
            "contract_template_show" => Command::TemplateShow {
                name: s(a, "name")?.into(),
                version: int(a, "version")?,
            },
            "contract_draft" => Command::Draft(DraftReq {
                template: opt_s(a, "template"),
                title: s(a, "title")?.into(),
                counterparty: s(a, "counterparty")?.into(),
                values: values(a)?,
                body: source(a, "body", "body_path")?,
            }),
            "contract_list" => Command::List {
                status: opt_s(a, "status")
                    .map(|v| Status::parse(&v).map_err(e))
                    .transpose()?,
                text: opt_s(a, "text"),
            },
            "contract_show" => Command::Show {
                contract: s(a, "contract")?.into(),
                version: int(a, "version")?,
                as_of: opt_date(a, "as_of")?,
            },
            "contract_edit" => Command::Edit {
                contract: s(a, "contract")?.into(),
                body: source(a, "body", "body_path")?.ok_or("pass body or body_path")?,
                note: opt_s(a, "note").unwrap_or_default(),
            },
            "contract_terms" => Command::Terms {
                contract: s(a, "contract")?.into(),
                terms: Terms {
                    start: opt_date(a, "start")?,
                    initial_months: int(a, "initial_months")?,
                    renewal_months: int(a, "renewal_months")?.unwrap_or(0),
                    notice_months: int(a, "notice_months")?.unwrap_or(0),
                    annual_value_cents: match a.get("annual_value") {
                        None | Some(Value::Null) => None,
                        Some(Value::String(t)) => Some(
                            Money::parse_de(t)
                                .map(Money::cents)
                                .map_err(|x| format!("annual_value: {x}"))?,
                        ),
                        Some(_) => return Err(
                            "annual_value must be a DE amount string like \"9.600,00\" (no floats)"
                                .into(),
                        ),
                    },
                },
            },
            "contract_transition" => Command::Transition {
                contract: s(a, "contract")?.into(),
                to: Status::parse(s(a, "to")?).map_err(e)?,
                note: opt_s(a, "note").unwrap_or_default(),
            },
            "contract_seal" => Command::Seal {
                contract: s(a, "contract")?.into(),
            },
            "contract_verify" => Command::Verify {
                contract: s(a, "contract")?.into(),
                path: opt_s(a, "path"),
            },
            "contract_redline" => Command::Redline {
                contract: s(a, "contract")?.into(),
                from: int(a, "from")?,
                to: int(a, "to")?,
            },
            "contract_risk_scan" => Command::RiskScan {
                contract: opt_s(a, "contract"),
                text: opt_s(a, "text"),
                version: int(a, "version")?,
                rules_path: opt_s(a, "rules_path"),
            },
            "contract_render" => Command::Render {
                contract: s(a, "contract")?.into(),
                format: opt_s(a, "format")
                    .unwrap_or_else(|| "md".into())
                    .to_ascii_lowercase(),
                path: s(a, "path")?.into(),
                version: int(a, "version")?,
            },
            "contract_deadlines" => Command::Deadlines {
                within_days: int(a, "within_days")?.unwrap_or(90).clamp(0, 3650),
                as_of: opt_date(a, "as_of")?,
                emit: flag(a, "emit"),
            },
            "contract_demo_seed" => Command::DemoSeed,
            other => return Err(format!("unknown tool {other}")),
        })
    }
}

fn to_json<T: serde::Serialize>(v: &T) -> Result<Value, String> {
    serde_json::to_value(v).map_err(|x| x.to_string())
}

fn read_text(ctx: &Ctx, src: Source) -> Result<String, String> {
    match src {
        Source::Text(t) => Ok(t),
        Source::File(p) => {
            let path = ctx.confinement.check(&p, false)?;
            let size = std::fs::metadata(&path)
                .map_err(|x| format!("{}: {x}", path.display()))?
                .len();
            if size > MAX_FILE_BYTES {
                return Err("file larger than 2 MB".into());
            }
            std::fs::read_to_string(&path)
                .map(|t| t.trim_start_matches('\u{feff}').replace("\r\n", "\n"))
                .map_err(|x| format!("{}: {x}", path.display()))
        }
    }
}

/// Next date that matters: the last notice day of an auto-renewing contract, or
/// the end of a fixed term. None without terms or after a fixed term ended.
pub fn next_deadline(c: &Contract, today: NaiveDate) -> Result<Option<Value>, String> {
    let (Some(start), Some(initial)) = (c.terms.start, c.terms.initial_months) else {
        return Ok(None);
    };
    let to_u32 = |v: i64| u32::try_from(v).map_err(|_| "bad term length".to_string());
    if c.terms.renewal_months > 0 {
        let ar = AutoRenewal {
            start,
            initial_months: to_u32(initial)?,
            renewal_months: to_u32(c.terms.renewal_months)?,
            notice_months: to_u32(c.terms.notice_months)?,
        };
        let nd = ar.next_deadline(today).map_err(|x| x.to_string())?;
        return Ok(Some(
            json!({ "kind": "notice", "due": nd.last_notice, "term_end": nd.term_end, "days_left": (nd.last_notice - today).num_days() }),
        ));
    }
    let end = start
        .checked_add_months(Months::new(to_u32(initial)?))
        .and_then(|d| d.pred_opt())
        .ok_or("bad term")?;
    if end < today {
        return Ok(None);
    }
    Ok(Some(
        json!({ "kind": "expiry", "due": end, "term_end": end, "days_left": (end - today).num_days() }),
    ))
}

fn contract_json(c: &Contract, today: NaiveDate) -> Result<Value, String> {
    let mut v = to_json(c)?;
    v["next"] = json!(c
        .status
        .next()
        .iter()
        .map(|s| s.as_str())
        .collect::<Vec<_>>());
    v["annual_value"] = json!(c.terms.annual_value_cents.map(eur));
    v["deadline"] = next_deadline(c, today)?.unwrap_or(Value::Null);
    Ok(v)
}

pub fn dispatch(ctx: &mut Ctx, cmd: Command) -> Result<Value, String> {
    match cmd {
        Command::Status => Ok(
            json!({ "app": APP, "version": VERSION, "stats": to_json(&ctx.db.stats().map_err(e)?)?, "seal_key": ctx.key_path.exists() }),
        ),
        Command::TemplateAdd {
            name,
            description,
            variables,
            body,
        } => {
            let text = read_text(ctx, body)?;
            to_json(
                &ctx.db
                    .add_template(&name, &description, &variables, &text)
                    .map_err(e)?,
            )
        }
        Command::TemplateList => {
            let list: Vec<Value> = ctx.db.templates().map_err(e)?.iter().map(|t| json!({ "name": t.name, "version": t.version, "description": t.description, "variables": t.variables.iter().map(|v| v.name.clone()).collect::<Vec<_>>() })).collect();
            Ok(json!({ "templates": list }))
        }
        Command::TemplateShow { name, version } => {
            to_json(&ctx.db.template(&name, version).map_err(e)?)
        }
        Command::Draft(req) => draft(ctx, req, today()),
        Command::List { status, text } => {
            let t = today();
            let rows = ctx
                .db
                .contracts(status, text.as_deref())
                .map_err(e)?
                .iter()
                .map(|c| contract_json(c, t))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(json!({ "contracts": rows }))
        }
        Command::Show {
            contract,
            version,
            as_of,
        } => show(ctx, &contract, version, as_of.unwrap_or_else(today)),
        Command::Edit {
            contract,
            body,
            note,
        } => {
            let text = read_text(ctx, body)?;
            let c = ctx.db.edit(&contract, &text, &note).map_err(e)?;
            let mut v = contract_json(&c, today())?;
            v["redline"] = redline(
                ctx,
                &c.number,
                Some(c.current_version - 1),
                Some(c.current_version),
            )?["stats"]
                .clone();
            Ok(v)
        }
        Command::Terms { contract, terms } => {
            contract_json(&ctx.db.set_terms(&contract, &terms).map_err(e)?, today())
        }
        Command::Transition { contract, to, note } => transition(ctx, &contract, to, &note, true),
        Command::Seal { contract } => seal_contract(ctx, &contract),
        Command::Verify { contract, path } => verify(ctx, &contract, path.as_deref()),
        Command::Redline { contract, from, to } => redline(ctx, &contract, from, to),
        Command::RiskScan {
            contract,
            text,
            version,
            rules_path,
        } => risk_scan(
            ctx,
            contract.as_deref(),
            text,
            version,
            rules_path.as_deref(),
        ),
        Command::Render {
            contract,
            format,
            path,
            version,
        } => render_contract(ctx, &contract, &format, &path, version),
        Command::Deadlines {
            within_days,
            as_of,
            emit,
        } => deadlines(ctx, within_days, as_of.unwrap_or_else(today), emit),
        Command::DemoSeed => demo_seed(ctx),
    }
}

pub fn draft(ctx: &mut Ctx, req: DraftReq, today: NaiveDate) -> Result<Value, String> {
    let DraftReq {
        template,
        title,
        counterparty,
        values: vals,
        body,
    } = req;
    let (text, used, tpl) = match (template, body) {
        (Some(_), Some(_)) => return Err("pass template or body, not both".into()),
        (None, None) => {
            return Err("pass a template name (see contract_template_list) or a body".into())
        }
        (Some(name), None) => {
            let t = ctx.db.template(&name, None).map_err(e)?;
            let (text, used) = fill(&t, &vals).map_err(e)?;
            (text, used, Some((t.name, t.version)))
        }
        (None, Some(src)) => {
            if !vals.is_empty() {
                return Err("values only apply to templates".into());
            }
            (read_text(ctx, src)?, BTreeMap::new(), None)
        }
    };
    let c = ctx
        .db
        .create(
            &NewContract {
                title,
                counterparty,
                template: tpl,
                values: used,
                body: text,
            },
            &ctx.actor,
            today,
        )
        .map_err(e)?;
    let mut v = contract_json(&c, today)?;
    let findings = risk::scan(
        &risk::default_rules()?,
        &ctx.db.version(&c, None).map_err(e)?.body,
    );
    v["risk_findings"] = json!(findings.len());
    v["risk_high"] = json!(findings.iter().filter(|f| f.severity == "high").count());
    Ok(v)
}

fn show(ctx: &Ctx, key: &str, version: Option<i64>, as_of: NaiveDate) -> Result<Value, String> {
    let c = ctx.db.contract(key).map_err(e)?;
    let ver = ctx.db.version(&c, version).map_err(e)?;
    let mut v = contract_json(&c, as_of)?;
    v["text_version"] = json!(ver.version);
    v["text"] = json!(ver.body);
    v["versions"] = to_json(&ctx.db.versions(&c).map_err(e)?)?;
    v["history"] = to_json(&ctx.db.history(&c).map_err(e)?)?;
    v["seals"] = to_json(&ctx.db.seals(&c).map_err(e)?)?;
    v["risk"] = to_json(&risk::scan(&risk::default_rules()?, &ver.body))?;
    Ok(v)
}

/// Status change. Executing emits `contract.executed` to gaincraft once.
pub fn transition(
    ctx: &mut Ctx,
    key: &str,
    to: Status,
    note: &str,
    emit: bool,
) -> Result<Value, String> {
    let c = ctx.db.transition(key, to, &ctx.actor, note).map_err(e)?;
    let mut v = contract_json(&c, today())?;
    if to == Status::Executed
        && emit
        && ctx
            .db
            .emitted(&c, "contract.executed", "executed")
            .map_err(e)?
            .is_none()
    {
        let dl = next_deadline(&c, today())?;
        let payload = json!({
            "contract_id": c.number, "title": c.title, "counterparty": c.counterparty,
            "annual_value_cents": c.terms.annual_value_cents, "start": c.terms.start,
            "renewal_months": c.terms.renewal_months, "notice_months": c.terms.notice_months,
            "term_end": dl.as_ref().map(|d| d["term_end"].clone()), "last_notice": dl.as_ref().filter(|d| d["kind"] == "notice").map(|d| d["due"].clone()),
        });
        let ev = ctx
            .bus
            .emit(APP, "gaincraft", "contract.executed", payload)
            .map_err(|x| x.to_string())?;
        ctx.db
            .record_emitted(&c, "contract.executed", "executed", &ev.id)
            .map_err(e)?;
        v["emitted"] = json!({ "gaincraft": ev.id });
    }
    Ok(v)
}

fn seal_contract(ctx: &mut Ctx, key: &str) -> Result<Value, String> {
    let c = ctx.db.contract(key).map_err(e)?;
    if matches!(
        c.status,
        Status::Cancelled | Status::Terminated | Status::Expired
    ) {
        return Err(format!(
            "{} is {}; nothing to seal",
            c.number,
            c.status.as_str()
        ));
    }
    if ctx
        .db
        .seals(&c)
        .map_err(e)?
        .iter()
        .any(|s| s.version == c.current_version)
    {
        return Err(format!(
            "version {} of {} is already sealed",
            c.current_version, c.number
        ));
    }
    let ver = ctx.db.version(&c, None).map_err(e)?;
    let key = seal::load_or_create_key(&ctx.key_path)?;
    let at = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let sealed = seal::seal(&key, &c.number, ver.version, ver.body.as_bytes(), &at);
    let s = ctx
        .db
        .add_seal(
            &c,
            ver.version,
            &sealed.sha256,
            &at,
            &sealed.signature,
            &sealed.public_key,
        )
        .map_err(e)?;
    let mut v = to_json(&s)?;
    v["contract"] = json!(c.number);
    v["note"] =
        json!("Tamper evidence (SHA-256 + Ed25519), not a legally binding electronic signature.");
    Ok(v)
}

/// Verify every seal against the stored text; with `path`, also check that the
/// file's bytes are exactly the sealed text (e.g. a rendered .md sent to the other side).
fn verify(ctx: &Ctx, key: &str, path: Option<&str>) -> Result<Value, String> {
    let c = ctx.db.contract(key).map_err(e)?;
    let seals = ctx.db.seals(&c).map_err(e)?;
    if seals.is_empty() {
        return Err(format!("{} has no seal", c.number));
    }
    let mut rows = Vec::new();
    let mut all_ok = true;
    for s in &seals {
        let text = ctx.db.version(&c, Some(s.version)).map_err(e)?.body;
        let verdict = seal::verify(
            &c.number,
            s.version,
            text.as_bytes(),
            &s.sha256,
            &s.sealed_at,
            &s.signature,
            &s.public_key,
        )?;
        all_ok &= verdict.ok();
        rows.push(json!({ "version": s.version, "sealed_at": s.sealed_at, "ok": verdict.ok(), "hash_matches": verdict.hash_matches, "signature_valid": verdict.signature_valid }));
    }
    let mut out = json!({ "contract": c.number, "ok": all_ok, "rows": rows });
    if let Some(p) = path {
        let file = ctx.confinement.check(p, false)?;
        let bytes = std::fs::read(&file).map_err(|x| format!("{}: {x}", file.display()))?;
        let latest = seals.last().ok_or("no seal")?;
        let verdict = seal::verify(
            &c.number,
            latest.version,
            &bytes,
            &latest.sha256,
            &latest.sealed_at,
            &latest.signature,
            &latest.public_key,
        )?;
        out["file"] = json!({ "path": file.display().to_string(), "version": latest.version, "matches_seal": verdict.ok(), "sha256": verdict.actual_sha256 });
        if !verdict.ok() {
            out["ok"] = json!(false);
        }
    }
    Ok(out)
}

pub fn redline(ctx: &Ctx, key: &str, from: Option<i64>, to: Option<i64>) -> Result<Value, String> {
    let c = ctx.db.contract(key).map_err(e)?;
    let to = to.unwrap_or(c.current_version);
    let from = from.unwrap_or(to - 1);
    if from < 1 || from == to {
        return Err(format!(
            "{} has no earlier version to compare (versions 1-{})",
            c.number, c.current_version
        ));
    }
    let a = ctx.db.version(&c, Some(from)).map_err(e)?.body;
    let b = ctx.db.version(&c, Some(to)).map_err(e)?.body;
    let diff = TextDiff::from_lines(&a, &b);
    let (mut ins, mut del) = (0, 0);
    let mut changes = Vec::new();
    for ch in diff.iter_all_changes() {
        let (tag, sign) = match ch.tag() {
            ChangeTag::Delete => {
                del += 1;
                ("delete", "-")
            }
            ChangeTag::Insert => {
                ins += 1;
                ("insert", "+")
            }
            ChangeTag::Equal => continue,
        };
        changes.push(
            json!({ "op": tag, "line": format!("{sign} {}", ch.value().trim_end_matches('\n')) }),
        );
    }
    let unified = diff
        .unified_diff()
        .context_radius(2)
        .header(
            &format!("{} v{from}", c.number),
            &format!("{} v{to}", c.number),
        )
        .to_string();
    // Risk delta: what the new text adds or removes.
    let rules = risk::default_rules()?;
    let before: Vec<String> = risk::scan(&rules, &a).into_iter().map(|f| f.rule).collect();
    let after: Vec<String> = risk::scan(&rules, &b).into_iter().map(|f| f.rule).collect();
    let added: Vec<&String> = after.iter().filter(|r| !before.contains(r)).collect();
    let removed: Vec<&String> = before.iter().filter(|r| !after.contains(r)).collect();
    Ok(json!({
        "contract": c.number, "from": from, "to": to,
        "stats": { "lines_inserted": ins, "lines_deleted": del, "risk_added": added, "risk_removed": removed },
        "rows": changes, "unified": unified,
    }))
}

fn risk_scan(
    ctx: &Ctx,
    key: Option<&str>,
    text: Option<String>,
    version: Option<i64>,
    rules_path: Option<&str>,
) -> Result<Value, String> {
    let rules = match rules_path {
        Some(p) => {
            let path = ctx.confinement.check(p, false)?;
            risk::load(
                &std::fs::read_to_string(&path).map_err(|x| format!("{}: {x}", path.display()))?,
            )?
        }
        None => risk::default_rules()?,
    };
    let (label, body) = match (key, text) {
        (Some(k), None) => {
            let c = ctx.db.contract(k).map_err(e)?;
            let v = ctx.db.version(&c, version).map_err(e)?;
            (format!("{} v{}", c.number, v.version), v.body)
        }
        (None, Some(t)) => ("text".to_string(), t),
        _ => return Err("pass contract or text".into()),
    };
    let findings = risk::scan(&rules, &body);
    Ok(json!({
        "scanned": label, "rules": rules.len(), "findings": findings.len(),
        "high": findings.iter().filter(|f| f.severity == "high").count(),
        "disclaimer": "Deterministic pattern scan to direct a human review; not legal advice.",
        "rows": to_json(&findings)?,
    }))
}

fn render_contract(
    ctx: &Ctx,
    key: &str,
    format: &str,
    path: &str,
    version: Option<i64>,
) -> Result<Value, String> {
    let c = ctx.db.contract(key).map_err(e)?;
    let ver = ctx.db.version(&c, version).map_err(e)?;
    let out = ctx.confinement.check(path, true)?;
    match format {
        "md" | "txt" => std::fs::write(&out, ver.body.as_bytes())
            .map_err(|x| format!("{}: {x}", out.display()))?,
        "typ" => std::fs::write(
            &out,
            render::to_typst(
                &c.title,
                &format!("{} v{}", c.number, ver.version),
                &ver.body,
            ),
        )
        .map_err(|x| format!("{}: {x}", out.display()))?,
        "pdf" => {
            let src = out.with_extension("typ");
            std::fs::write(
                &src,
                render::to_typst(
                    &c.title,
                    &format!("{} v{}", c.number, ver.version),
                    &ver.body,
                ),
            )
            .map_err(|x| format!("{}: {x}", src.display()))?;
            render::compile_pdf(&src, &out)?;
        }
        other => return Err(format!("format {other:?}: use md, typ or pdf")),
    }
    let bytes = std::fs::read(&out).map_err(|x| format!("{}: {x}", out.display()))?;
    let sealed = ctx
        .db
        .seals(&c)
        .map_err(e)?
        .into_iter()
        .find(|s| s.version == ver.version);
    Ok(json!({
        "contract": c.number, "version": ver.version, "format": format, "written": out.display().to_string(),
        "bytes": bytes.len(), "sha256": seal::sha256_hex(&bytes),
        "matches_seal": sealed.map(|s| s.sha256 == seal::sha256_hex(&bytes)),
    }))
}

/// Upcoming notice deadlines and term ends of executed contracts. With `emit`,
/// each deadline is sent once to taskcraft as `contract.deadline`.
pub fn deadlines(
    ctx: &mut Ctx,
    within_days: i64,
    as_of: NaiveDate,
    emit: bool,
) -> Result<Value, String> {
    let mut rows = Vec::new();
    let mut emitted = 0;
    for c in ctx.db.contracts(Some(Status::Executed), None).map_err(e)? {
        let Some(dl) = next_deadline(&c, as_of)? else {
            rows.push(json!({ "contract": c.number, "title": c.title, "counterparty": c.counterparty, "kind": "term_ended", "due": Value::Null, "days_left": Value::Null, "hint": "fixed term is over: transition to expired" }));
            continue;
        };
        let days = dl["days_left"].as_i64().unwrap_or(i64::MAX);
        if days > within_days {
            continue;
        }
        let due = dl["due"].as_str().unwrap_or_default().to_string();
        let kind = dl["kind"].as_str().unwrap_or_default().to_string();
        let mut sent = ctx
            .db
            .emitted(&c, "contract.deadline", &format!("{kind}:{due}"))
            .map_err(e)?;
        if emit && sent.is_none() {
            let title = if kind == "notice" {
                format!("Kündigungsfrist {} ({})", c.title, c.counterparty)
            } else {
                format!("Vertragsende {} ({})", c.title, c.counterparty)
            };
            let payload = json!({ "contract_id": c.number, "title": title, "due_date": due, "term_end": dl["term_end"], "kind": kind, "counterparty": c.counterparty });
            let ev = ctx
                .bus
                .emit(APP, "taskcraft", "contract.deadline", payload)
                .map_err(|x| x.to_string())?;
            ctx.db
                .record_emitted(&c, "contract.deadline", &format!("{kind}:{due}"), &ev.id)
                .map_err(e)?;
            emitted += 1;
            sent = Some(ev.id);
        }
        rows.push(json!({ "contract": c.number, "title": c.title, "counterparty": c.counterparty, "kind": kind, "due": due, "term_end": dl["term_end"], "days_left": days, "sent_to_taskcraft": sent }));
    }
    Ok(json!({ "as_of": as_of, "within_days": within_days, "emitted": emitted, "rows": rows }))
}

const DEMO_SERVICE: &str = "# Wartungsvertrag

Zwischen {{kunde}} (Auftraggeber) und Joe Mocky Consulting e.U. (Auftragnehmer).

## § 1 Gegenstand
Der Auftragnehmer erbringt {{leistung}}.

## § 2 Entgelt
Das Entgelt beträgt {{entgelt}} netto pro Monat, zahlbar binnen 14 Tagen.

## § 3 Laufzeit
Der Vertrag beginnt am {{beginn}} und wird für zwölf Monate abgeschlossen. Er verlängert sich automatisch um jeweils zwölf Monate, wenn er nicht unter Einhaltung einer Frist von {{kuendigungsfrist}} zum Ende der Laufzeit gekündigt wird.

## § 4 Haftung
Die Haftung ist auf Vorsatz und grobe Fahrlässigkeit beschränkt und mit dem Jahresentgelt begrenzt.

## § 5 Schlussbestimmungen
Es gilt österreichisches Recht. Gerichtsstand ist {{ort}}.
";

const DEMO_NDA: &str = "# Mutual Non-Disclosure Agreement

Between {{party}} and Joe Mocky Consulting e.U.

## 1. Purpose
The parties exchange confidential information for {{purpose}}.

## 2. Obligations
Each party shall indemnify and hold harmless the other party against any breach of this agreement.

## 3. Personal data
Information may include personal data of employees of either party.

## 4. Term
This agreement renews automatically each year.

## 5. Law and venue
This agreement is governed by the laws of the State of New York. Venue: {{venue}}.
";

/// Labelled demo templates and contracts. Only on an empty database.
pub fn demo_seed(ctx: &mut Ctx) -> Result<Value, String> {
    if !ctx.db.templates().map_err(e)?.is_empty()
        || !ctx.db.contracts(None, None).map_err(e)?.is_empty()
    {
        return Err("demo seed only runs on an empty database".into());
    }
    let day = NaiveDate::from_ymd_opt(2026, 11, 9).ok_or("bad demo date")?;
    let var = |n: &str, req: bool, def: Option<&str>, d: &str| Variable {
        name: n.into(),
        required: req,
        default: def.map(str::to_string),
        description: d.into(),
    };
    ctx.db.set_meta("demo", "1").map_err(e)?;
    ctx.db
        .add_template(
            "Wartungsvertrag (DEMO)",
            "Österreichischer Wartungsvertrag mit automatischer Verlängerung",
            &[
                var("kunde", true, None, "Auftraggeber mit Rechtsform"),
                var("leistung", true, None, "Leistungsbeschreibung"),
                var("entgelt", true, None, "Monatsentgelt, z.B. 800,00 EUR"),
                var("beginn", true, None, "Vertragsbeginn"),
                var(
                    "kuendigungsfrist",
                    false,
                    Some("drei Monaten"),
                    "Kündigungsfrist",
                ),
                var("ort", false, Some("Wien"), "Gerichtsstand"),
            ],
            DEMO_SERVICE,
        )
        .map_err(e)?;
    ctx.db
        .add_template(
            "NDA (DEMO)",
            "English mutual NDA, deliberately risky for the scanner demo",
            &[
                var("party", true, None, "Other party"),
                var("purpose", true, None, "Purpose"),
                var("venue", true, None, "Venue"),
            ],
            DEMO_NDA,
        )
        .map_err(e)?;
    let vals = |pairs: &[(&str, &str)]| {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect::<BTreeMap<_, _>>()
    };
    // 1: executed service contract with a notice deadline coming up.
    let c1 = draft(
        ctx,
        DraftReq {
            template: Some("Wartungsvertrag (DEMO)".into()),
            title: "Wartung Server und Backup".into(),
            counterparty: "Mock Software GmbH (DEMO)".into(),
            values: vals(&[
                ("kunde", "Mock Software GmbH (DEMO)"),
                ("leistung", "Wartung von Server und Backup"),
                ("entgelt", "800,00 EUR"),
                ("beginn", "01.03.2025"),
            ]),
            body: None,
        },
        day,
    )?;
    let n1 = c1["number"].as_str().ok_or("no number")?.to_string();
    let terms = Terms {
        start: NaiveDate::from_ymd_opt(2025, 3, 1),
        initial_months: Some(12),
        renewal_months: 12,
        notice_months: 3,
        annual_value_cents: Some(960_000),
    };
    ctx.db.set_terms(&n1, &terms).map_err(e)?;
    transition(ctx, &n1, Status::UnderReview, "", false)?;
    transition(ctx, &n1, Status::PendingSignature, "", false)?;
    seal_contract(ctx, &n1)?;
    transition(
        ctx,
        &n1,
        Status::Executed,
        "beidseitig unterschrieben (DEMO)",
        false,
    )?;
    // 2: risky NDA draft.
    draft(
        ctx,
        DraftReq {
            template: Some("NDA (DEMO)".into()),
            title: "NDA Projekt Falke".into(),
            counterparty: "Mockingbird Inc. (DEMO)".into(),
            values: vals(&[
                ("party", "Mockingbird Inc. (DEMO)"),
                ("purpose", "evaluating a joint product"),
                ("venue", "New York"),
            ]),
            body: None,
        },
        day,
    )?;
    // 3: second service contract, renegotiated: v2 widens liability (redline + risk delta).
    let c3 = draft(
        ctx,
        DraftReq {
            template: Some("Wartungsvertrag (DEMO)".into()),
            title: "Wartung Webshop".into(),
            counterparty: "Hannes Mockinger e.U. (DEMO)".into(),
            values: vals(&[
                ("kunde", "Hannes Mockinger e.U. (DEMO)"),
                ("leistung", "Wartung des Webshops"),
                ("entgelt", "450,00 EUR"),
                ("beginn", "01.01.2027"),
                ("ort", "Graz"),
            ]),
            body: None,
        },
        day,
    )?;
    let n3 = c3["number"].as_str().ok_or("no number")?.to_string();
    let v1 = ctx
        .db
        .version(&ctx.db.contract(&n3).map_err(e)?, None)
        .map_err(e)?
        .body;
    let v2 = v1.replace(
        "Die Haftung ist auf Vorsatz und grobe Fahrlässigkeit beschränkt und mit dem Jahresentgelt begrenzt.",
        "Der Auftragnehmer haftet unbeschränkt für alle Schäden, auch für Folgeschäden.",
    );
    ctx.db
        .edit(&n3, &v2, "Kunde verlangt volle Haftung (DEMO)")
        .map_err(e)?;
    transition(ctx, &n3, Status::UnderReview, "", false)?;
    Ok(json!({ "demo": true, "stats": to_json(&ctx.db.stats().map_err(e)?)? }))
}

/// MCP tool catalog.
pub fn tools() -> Vec<Value> {
    let c = json!({ "type": "string", "description": "Contract number like C-2026-001 (or id)" });
    let date = json!({ "type": "string", "description": "DD.MM.YYYY or YYYY-MM-DD" });
    let status = json!({ "type": "string", "enum": ["draft", "under_review", "pending_signature", "executed", "terminated", "expired", "cancelled"] });
    vec![
        tool("contract_status", "Counts by status, templates, sealed contracts, whether the seal key exists.", json!({}), &[]),
        tool("contract_template_add", "Add a template (or a new version of it). Placeholders {{name}} must match the declared variables exactly.", json!({ "name": {"type":"string"}, "description": {"type":"string"}, "variables": {"type":"array","items":{"type":"object","properties":{"name":{"type":"string"},"required":{"type":"boolean"},"default":{"type":"string"},"description":{"type":"string"}},"required":["name"]}}, "body": {"type":"string"}, "body_path": {"type":"string"} }), &["name"]),
        tool("contract_template_list", "Templates (latest versions) with their variables.", json!({}), &[]),
        tool("contract_template_show", "One template with body and variables.", json!({ "name": {"type":"string"}, "version": {"type":"integer"} }), &["name"]),
        tool("contract_draft", "Create a draft from a template (values fill its variables; typos and missing required values are errors) or from your own text. Returns a quick risk count.", json!({ "template": {"type":"string"}, "title": {"type":"string"}, "counterparty": {"type":"string"}, "values": {"type":"object"}, "body": {"type":"string"}, "body_path": {"type":"string"} }), &["title", "counterparty"]),
        tool("contract_list", "Contracts with status, allowed next statuses and the next deadline.", json!({ "status": status, "text": {"type":"string"} }), &[]),
        tool("contract_show", "One contract: text (current or a version), versions, status history, seals, risk findings, next deadline.", json!({ "contract": c, "version": {"type":"integer"}, "as_of": date }), &["contract"]),
        tool("contract_edit", "New text version (draft only). Returns the redline stats and risk delta.", json!({ "contract": c, "body": {"type":"string"}, "body_path": {"type":"string"}, "note": {"type":"string"} }), &["contract"]),
        tool("contract_terms", "Term data for deadlines: start, initial_months, renewal_months (0 = fixed term), notice_months, annual_value. Fixed after execution.", json!({ "contract": c, "start": date, "initial_months": {"type":"integer"}, "renewal_months": {"type":"integer"}, "notice_months": {"type":"integer"}, "annual_value": {"type":"string","description":"DE amount"} }), &["contract"]),
        tool("contract_transition", "Move along draft -> under_review -> pending_signature -> executed -> terminated/expired (cancelled from any open state). Executing needs a seal of the current version and sends contract.executed to gaincraft.", json!({ "contract": c, "to": status, "note": {"type":"string"} }), &["contract", "to"]),
        tool("contract_seal", "Seal the current version: SHA-256 of the text + Ed25519 signature. Tamper evidence, not a legal e-signature.", json!({ "contract": c }), &["contract"]),
        tool("contract_verify", "Check all seals against the stored text; with path, check that a file is byte-identical to the sealed text.", json!({ "contract": c, "path": {"type":"string"} }), &["contract"]),
        tool("contract_redline", "Line diff between two versions (default: previous vs current) plus which risk findings were added or removed.", json!({ "contract": c, "from": {"type":"integer"}, "to": {"type":"integer"} }), &["contract"]),
        tool("contract_risk_scan", "Deterministic DE/EN clause scan (liability, indemnity, auto-renewal, foreign venue/law, missing DPA, penalties, unilateral changes, exclusivity). Optional TOML rule file. Not legal advice.", json!({ "contract": c, "text": {"type":"string"}, "version": {"type":"integer"}, "rules_path": {"type":"string"} }), &[]),
        tool("contract_render", "Write the text as md (byte-identical to the sealed text), typ (Typst source) or pdf (needs the Typst CLI; fails clearly without it).", json!({ "contract": c, "format": {"type":"string","enum":["md","typ","pdf"]}, "path": {"type":"string"}, "version": {"type":"integer"} }), &["contract", "path"]),
        tool("contract_deadlines", "Notice deadlines and term ends of executed contracts within N days (default 90). emit=true sends each once to taskcraft as contract.deadline.", json!({ "within_days": {"type":"integer"}, "as_of": date, "emit": {"type":"boolean"} }), &[]),
        tool("contract_demo_seed", "Fill an EMPTY database with labelled demo templates and three contracts (executed, risky draft, renegotiated).", json!({}), &[]),
    ]
}

#[cfg(test)]
mod tests;
