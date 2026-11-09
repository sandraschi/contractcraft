//! contractcraft-store: templates, contracts, versions, seals.
//!
//! Invariants:
//! - Status moves only along the allowed edges (`Status::can_move_to`); every
//!   move is logged with actor and note.
//! - Contract text changes only by adding a new version, and only in `draft`.
//!   Versions and seals are immutable (SQLite triggers refuse UPDATE/DELETE,
//!   and a trigger refuses new versions outside draft).
//! - Executing requires a seal of the current version.
//! - Template versions are kept: editing a template adds a version; contracts
//!   record which version they came from.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use chrono::{Datelike, NaiveDate, Utc};
use rusqlite::types::Value as SqlValue;
use rusqlite::{params, params_from_iter, Connection, OptionalExtension, Row};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

pub const SCHEMA_VERSION: i64 = 1;
pub const MAX_BODY: usize = 500_000;

#[derive(Debug, thiserror::Error)]
pub enum ContractError {
    #[error("database: {0}")]
    Db(String),
    #[error("invalid: {0}")]
    Invalid(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("not allowed: {0}")]
    State(String),
}

impl From<rusqlite::Error> for ContractError {
    fn from(e: rusqlite::Error) -> Self {
        let s = e.to_string();
        if s.contains("immutable") || s.contains("only in draft") {
            ContractError::State(s)
        } else {
            ContractError::Db(s)
        }
    }
}

pub type Result<T> = std::result::Result<T, ContractError>;

fn now() -> String {
    Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Draft,
    UnderReview,
    PendingSignature,
    Executed,
    Terminated,
    Expired,
    Cancelled,
}

impl Status {
    pub const ALL: [Status; 7] = [
        Status::Draft,
        Status::UnderReview,
        Status::PendingSignature,
        Status::Executed,
        Status::Terminated,
        Status::Expired,
        Status::Cancelled,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Status::Draft => "draft",
            Status::UnderReview => "under_review",
            Status::PendingSignature => "pending_signature",
            Status::Executed => "executed",
            Status::Terminated => "terminated",
            Status::Expired => "expired",
            Status::Cancelled => "cancelled",
        }
    }

    pub fn parse(s: &str) -> Result<Status> {
        let t = s.trim().to_ascii_lowercase().replace([' ', '-'], "_");
        Status::ALL
            .iter()
            .copied()
            .find(|v| v.as_str() == t)
            .ok_or_else(|| {
                ContractError::Invalid(format!(
                    "unknown status {s:?} (one of: {})",
                    Status::ALL.map(|v| v.as_str()).join(", ")
                ))
            })
    }

    /// The status machine. Anything not listed is refused.
    pub fn can_move_to(self, to: Status) -> bool {
        use Status::*;
        matches!(
            (self, to),
            (Draft, UnderReview)
                | (Draft, Cancelled)
                | (UnderReview, Draft)
                | (UnderReview, PendingSignature)
                | (UnderReview, Cancelled)
                | (PendingSignature, Draft)
                | (PendingSignature, Executed)
                | (PendingSignature, Cancelled)
                | (Executed, Terminated)
                | (Executed, Expired)
        )
    }

    pub fn next(self) -> Vec<Status> {
        Status::ALL
            .iter()
            .copied()
            .filter(|t| self.can_move_to(*t))
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Variable {
    pub name: String,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub default: Option<String>,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Template {
    pub name: String,
    pub version: i64,
    pub description: String,
    pub variables: Vec<Variable>,
    pub body: String,
    pub created_at: String,
}

/// Term data for deadlines. `renewal_months == 0` means a fixed term.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
pub struct Terms {
    pub start: Option<NaiveDate>,
    pub initial_months: Option<i64>,
    pub renewal_months: i64,
    pub notice_months: i64,
    pub annual_value_cents: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Contract {
    pub id: i64,
    pub number: String,
    pub title: String,
    pub counterparty: String,
    pub template: Option<String>,
    pub template_version: Option<i64>,
    pub status: Status,
    pub values: BTreeMap<String, String>,
    pub terms: Terms,
    pub current_version: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Default)]
pub struct NewContract {
    pub title: String,
    pub counterparty: String,
    /// (template name, version) the text came from.
    pub template: Option<(String, i64)>,
    pub values: BTreeMap<String, String>,
    pub body: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Version {
    pub version: i64,
    pub body: String,
    pub note: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Transition {
    pub from: String,
    pub to: String,
    pub actor: String,
    pub note: String,
    pub at: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Seal {
    pub id: i64,
    pub version: i64,
    pub sha256: String,
    pub sealed_at: String,
    pub signature: String,
    pub public_key: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Stats {
    pub templates: i64,
    pub contracts: BTreeMap<String, i64>,
    pub sealed: i64,
    pub demo: bool,
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS templates (
    name TEXT NOT NULL,
    version INTEGER NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    variables TEXT NOT NULL,
    body TEXT NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY (name, version)
);
CREATE TABLE IF NOT EXISTS contracts (
    id INTEGER PRIMARY KEY,
    number TEXT NOT NULL UNIQUE,
    title TEXT NOT NULL,
    counterparty TEXT NOT NULL,
    template_name TEXT,
    template_version INTEGER,
    status TEXT NOT NULL,
    values_json TEXT NOT NULL DEFAULT '{}',
    start_date TEXT,
    initial_months INTEGER CHECK (initial_months IS NULL OR initial_months BETWEEN 1 AND 1200),
    renewal_months INTEGER NOT NULL DEFAULT 0 CHECK (renewal_months BETWEEN 0 AND 1200),
    notice_months INTEGER NOT NULL DEFAULT 0 CHECK (notice_months BETWEEN 0 AND 120),
    annual_value_cents INTEGER CHECK (annual_value_cents IS NULL OR annual_value_cents >= 0),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS contract_versions (
    contract_id INTEGER NOT NULL REFERENCES contracts(id),
    version INTEGER NOT NULL,
    body TEXT NOT NULL,
    note TEXT NOT NULL DEFAULT '',
    created_at TEXT NOT NULL,
    PRIMARY KEY (contract_id, version)
);
CREATE TRIGGER IF NOT EXISTS trg_versions_draft_only BEFORE INSERT ON contract_versions
WHEN (SELECT status FROM contracts WHERE id = NEW.contract_id) <> 'draft'
BEGIN SELECT RAISE(ABORT, 'contract text changes only in draft'); END;
CREATE TRIGGER IF NOT EXISTS trg_versions_no_update BEFORE UPDATE ON contract_versions
BEGIN SELECT RAISE(ABORT, 'contract versions are immutable'); END;
CREATE TRIGGER IF NOT EXISTS trg_versions_no_delete BEFORE DELETE ON contract_versions
BEGIN SELECT RAISE(ABORT, 'contract versions are immutable'); END;
CREATE TABLE IF NOT EXISTS transitions (
    id INTEGER PRIMARY KEY,
    contract_id INTEGER NOT NULL REFERENCES contracts(id),
    from_status TEXT NOT NULL,
    to_status TEXT NOT NULL,
    actor TEXT NOT NULL,
    note TEXT NOT NULL DEFAULT '',
    at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS seals (
    id INTEGER PRIMARY KEY,
    contract_id INTEGER NOT NULL REFERENCES contracts(id),
    version INTEGER NOT NULL,
    sha256 TEXT NOT NULL,
    sealed_at TEXT NOT NULL,
    signature TEXT NOT NULL,
    public_key TEXT NOT NULL
);
CREATE TRIGGER IF NOT EXISTS trg_seals_no_update BEFORE UPDATE ON seals
BEGIN SELECT RAISE(ABORT, 'seals are immutable'); END;
CREATE TRIGGER IF NOT EXISTS trg_seals_no_delete BEFORE DELETE ON seals
BEGIN SELECT RAISE(ABORT, 'seals are immutable'); END;
CREATE TABLE IF NOT EXISTS emitted (
    contract_id INTEGER NOT NULL REFERENCES contracts(id),
    kind TEXT NOT NULL,
    key TEXT NOT NULL,
    event_id TEXT NOT NULL,
    at TEXT NOT NULL,
    PRIMARY KEY (contract_id, kind, key)
);
";

const CONTRACT_COLS: &str = "c.id, c.number, c.title, c.counterparty, c.template_name, c.template_version, c.status, c.values_json, c.start_date, c.initial_months,
    c.renewal_months, c.notice_months, c.annual_value_cents, c.created_at, c.updated_at,
    (SELECT MAX(v.version) FROM contract_versions v WHERE v.contract_id = c.id)";

type ContractRaw = (
    i64,
    String,
    String,
    String,
    Option<String>,
    Option<i64>,
    String,
    String,
    Option<NaiveDate>,
    Option<i64>,
    i64,
    i64,
    Option<i64>,
    String,
    String,
    i64,
);

fn contract_raw(r: &Row) -> rusqlite::Result<ContractRaw> {
    Ok((
        r.get(0)?,
        r.get(1)?,
        r.get(2)?,
        r.get(3)?,
        r.get(4)?,
        r.get(5)?,
        r.get(6)?,
        r.get(7)?,
        r.get(8)?,
        r.get(9)?,
        r.get(10)?,
        r.get(11)?,
        r.get(12)?,
        r.get(13)?,
        r.get(14)?,
        r.get::<_, Option<i64>>(15)?.unwrap_or(0),
    ))
}

fn contract_from(raw: ContractRaw) -> Result<Contract> {
    let (
        id,
        number,
        title,
        counterparty,
        template,
        template_version,
        status,
        values_json,
        start,
        initial,
        renewal,
        notice,
        value,
        created_at,
        updated_at,
        current_version,
    ) = raw;
    Ok(Contract {
        id,
        number,
        title,
        counterparty,
        template,
        template_version,
        status: Status::parse(&status)?,
        values: serde_json::from_str(&values_json)
            .map_err(|e| ContractError::Db(format!("values of contract {id}: {e}")))?,
        terms: Terms {
            start,
            initial_months: initial,
            renewal_months: renewal,
            notice_months: notice,
            annual_value_cents: value,
        },
        current_version,
        created_at,
        updated_at,
    })
}

fn template_from(r: &Row) -> rusqlite::Result<(String, i64, String, String, String, String)> {
    Ok((
        r.get(0)?,
        r.get(1)?,
        r.get(2)?,
        r.get(3)?,
        r.get(4)?,
        r.get(5)?,
    ))
}

fn check_text(label: &str, s: &str, max: usize) -> Result<String> {
    let t = s.trim();
    if t.is_empty() || t.chars().count() > max {
        return Err(ContractError::Invalid(format!(
            "{label} must be 1-{max} characters"
        )));
    }
    Ok(t.to_string())
}

pub fn validate_body(body: &str) -> Result<()> {
    if body.trim().is_empty() {
        return Err(ContractError::Invalid("contract text is empty".into()));
    }
    if body.len() > MAX_BODY {
        return Err(ContractError::Invalid(format!(
            "contract text over {MAX_BODY} bytes"
        )));
    }
    Ok(())
}

pub struct Contracts {
    conn: Connection,
}

impl Contracts {
    pub fn open(path: &Path) -> Result<Contracts> {
        if let Some(dir) = path.parent() {
            if !dir.as_os_str().is_empty() {
                std::fs::create_dir_all(dir).map_err(|e| ContractError::Db(e.to_string()))?;
            }
        }
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL;")?;
        Self::init(conn)
    }

    pub fn open_in_memory() -> Result<Contracts> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Contracts> {
        conn.execute_batch("PRAGMA foreign_keys = ON; PRAGMA busy_timeout = 5000;")?;
        conn.execute_batch(SCHEMA)?;
        let v: Option<String> = conn
            .query_row(
                "SELECT value FROM meta WHERE key = 'schema_version'",
                [],
                |r| r.get(0),
            )
            .optional()?;
        match v.as_deref() {
            None => {
                conn.execute(
                    "INSERT INTO meta (key, value) VALUES ('schema_version', ?1)",
                    params![SCHEMA_VERSION.to_string()],
                )?;
            }
            Some(x) if x == SCHEMA_VERSION.to_string() => {}
            Some(x) => {
                return Err(ContractError::Db(format!(
                    "database schema {x} is newer than this build ({SCHEMA_VERSION})"
                )))
            }
        }
        Ok(Contracts { conn })
    }

    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    pub fn set_meta(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute("INSERT INTO meta (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value", params![key, value])?;
        Ok(())
    }

    pub fn is_demo(&self) -> Result<bool> {
        Ok(self
            .conn
            .query_row("SELECT value FROM meta WHERE key = 'demo'", [], |r| {
                r.get::<_, String>(0)
            })
            .optional()?
            .as_deref()
            == Some("1"))
    }

    // ---------- templates ----------

    /// Add a template, or a new version of an existing one. Placeholders `{{name}}`
    /// must match the declared variables exactly.
    pub fn add_template(
        &self,
        name: &str,
        description: &str,
        variables: &[Variable],
        body: &str,
    ) -> Result<Template> {
        let name = check_text("template name", name, 120)?;
        validate_body(body)?;
        let used = placeholders(body)?;
        let mut declared: Vec<&str> = Vec::new();
        for v in variables {
            if v.name.is_empty()
                || !v
                    .name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_')
            {
                return Err(ContractError::Invalid(format!(
                    "variable name {:?}: use a-z, 0-9, _",
                    v.name
                )));
            }
            if declared.contains(&v.name.as_str()) {
                return Err(ContractError::Invalid(format!(
                    "variable {} declared twice",
                    v.name
                )));
            }
            declared.push(&v.name);
        }
        if let Some(u) = used.iter().find(|u| !declared.contains(&u.as_str())) {
            return Err(ContractError::Invalid(format!(
                "placeholder {{{{{u}}}}} is not declared"
            )));
        }
        if let Some(d) = declared.iter().find(|d| !used.iter().any(|u| u == *d)) {
            return Err(ContractError::Invalid(format!(
                "variable {d} is declared but never used"
            )));
        }
        let version: i64 = self.conn.query_row(
            "SELECT COALESCE(MAX(version), 0) + 1 FROM templates WHERE name = ?1",
            params![name],
            |r| r.get(0),
        )?;
        let vars =
            serde_json::to_string(variables).map_err(|e| ContractError::Invalid(e.to_string()))?;
        self.conn.execute(
            "INSERT INTO templates (name, version, description, variables, body, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![name, version, description.trim(), vars, body, now()],
        )?;
        self.template(&name, Some(version))
    }

    /// Latest version unless `version` is given.
    pub fn template(&self, name: &str, version: Option<i64>) -> Result<Template> {
        let raw = match version {
            Some(v) => self
                .conn
                .query_row("SELECT name, version, description, variables, body, created_at FROM templates WHERE name = ?1 AND version = ?2", params![name.trim(), v], template_from)
                .optional()?,
            None => self
                .conn
                .query_row("SELECT name, version, description, variables, body, created_at FROM templates WHERE name = ?1 ORDER BY version DESC LIMIT 1", params![name.trim()], template_from)
                .optional()?,
        };
        let (name, version, description, vars, body, created_at) =
            raw.ok_or_else(|| ContractError::NotFound(format!("template {name}")))?;
        let variables = serde_json::from_str(&vars)
            .map_err(|e| ContractError::Db(format!("template variables: {e}")))?;
        Ok(Template {
            name,
            version,
            description,
            variables,
            body,
            created_at,
        })
    }

    pub fn templates(&self) -> Result<Vec<Template>> {
        let names: Vec<String> = {
            let mut st = self
                .conn
                .prepare("SELECT DISTINCT name FROM templates ORDER BY name")?;
            let v = st
                .query_map([], |r| r.get(0))?
                .collect::<rusqlite::Result<Vec<String>>>()?;
            v
        };
        names.iter().map(|n| self.template(n, None)).collect()
    }

    // ---------- contracts ----------

    fn next_number(&self, year: i32) -> Result<String> {
        let prefix = format!("C-{year}-");
        let n: i64 = self.conn.query_row(
            "SELECT COALESCE(MAX(CAST(substr(number, length(?1) + 1) AS INTEGER)), 0) + 1 FROM contracts WHERE number LIKE ?1 || '%'",
            params![prefix],
            |r| r.get(0),
        )?;
        Ok(format!("{prefix}{n:03}"))
    }

    pub fn create(&self, n: &NewContract, actor: &str, today: NaiveDate) -> Result<Contract> {
        let title = check_text("title", &n.title, 200)?;
        let counterparty = check_text("counterparty", &n.counterparty, 200)?;
        validate_body(&n.body)?;
        let number = self.next_number(today.year())?;
        let vals =
            serde_json::to_string(&n.values).map_err(|e| ContractError::Invalid(e.to_string()))?;
        let tx = self.conn.unchecked_transaction()?;
        let stamp = now();
        tx.execute(
            "INSERT INTO contracts (number, title, counterparty, template_name, template_version, status, values_json, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, 'draft', ?6, ?7, ?7)",
            params![number, title, counterparty, n.template.as_ref().map(|t| t.0.clone()), n.template.as_ref().map(|t| t.1), vals, stamp],
        )?;
        let id = tx.last_insert_rowid();
        tx.execute("INSERT INTO contract_versions (contract_id, version, body, note, created_at) VALUES (?1, 1, ?2, 'initial draft', ?3)", params![id, n.body, stamp])?;
        tx.execute("INSERT INTO transitions (contract_id, from_status, to_status, actor, note, at) VALUES (?1, '', 'draft', ?2, 'created', ?3)", params![id, actor, stamp])?;
        tx.commit()?;
        self.contract(&number)
    }

    /// By number (`C-2026-001`) or numeric id.
    pub fn contract(&self, key: &str) -> Result<Contract> {
        let k = key.trim().to_ascii_uppercase();
        let raw = match k.parse::<i64>() {
            Ok(id) => self
                .conn
                .query_row(
                    &format!("SELECT {CONTRACT_COLS} FROM contracts c WHERE c.id = ?1"),
                    params![id],
                    contract_raw,
                )
                .optional()?,
            Err(_) => self
                .conn
                .query_row(
                    &format!("SELECT {CONTRACT_COLS} FROM contracts c WHERE c.number = ?1"),
                    params![k],
                    contract_raw,
                )
                .optional()?,
        };
        contract_from(raw.ok_or_else(|| ContractError::NotFound(format!("contract {key}")))?)
    }

    pub fn contracts(&self, status: Option<Status>, text: Option<&str>) -> Result<Vec<Contract>> {
        let mut sql = format!("SELECT {CONTRACT_COLS} FROM contracts c WHERE 1 = 1");
        let mut args: Vec<SqlValue> = Vec::new();
        if let Some(s) = status {
            args.push(SqlValue::Text(s.as_str().into()));
            sql.push_str(&format!(" AND c.status = ?{}", args.len()));
        }
        if let Some(t) = text.map(str::trim).filter(|t| !t.is_empty()) {
            args.push(SqlValue::Text(format!("%{}%", t.to_lowercase())));
            let i = args.len();
            sql.push_str(&format!(" AND (lower(c.title) LIKE ?{i} OR lower(c.counterparty) LIKE ?{i} OR lower(c.number) LIKE ?{i})"));
        }
        sql.push_str(" ORDER BY c.number");
        let mut st = self.conn.prepare(&sql)?;
        let raws = st
            .query_map(params_from_iter(args.iter()), contract_raw)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        raws.into_iter().map(contract_from).collect()
    }

    pub fn version(&self, c: &Contract, version: Option<i64>) -> Result<Version> {
        let v = version.unwrap_or(c.current_version);
        self.conn
            .query_row("SELECT version, body, note, created_at FROM contract_versions WHERE contract_id = ?1 AND version = ?2", params![c.id, v], |r| {
                Ok(Version { version: r.get(0)?, body: r.get(1)?, note: r.get(2)?, created_at: r.get(3)? })
            })
            .optional()?
            .ok_or_else(|| ContractError::NotFound(format!("{} version {v}", c.number)))
    }

    pub fn versions(&self, c: &Contract) -> Result<Vec<Version>> {
        let mut st = self.conn.prepare("SELECT version, '', note, created_at FROM contract_versions WHERE contract_id = ?1 ORDER BY version")?;
        let rows = st
            .query_map(params![c.id], |r| {
                Ok(Version {
                    version: r.get(0)?,
                    body: r.get(1)?,
                    note: r.get(2)?,
                    created_at: r.get(3)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// New text version. Draft only; identical text is refused.
    pub fn edit(&self, key: &str, body: &str, note: &str) -> Result<Contract> {
        let c = self.contract(key)?;
        if c.status != Status::Draft {
            return Err(ContractError::State(format!(
                "{} is {}; move it back to draft to change the text",
                c.number,
                c.status.as_str()
            )));
        }
        validate_body(body)?;
        if self.version(&c, None)?.body == body {
            return Err(ContractError::Invalid("text is unchanged".into()));
        }
        self.conn.execute(
            "INSERT INTO contract_versions (contract_id, version, body, note, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![c.id, c.current_version + 1, body, note.trim(), now()],
        )?;
        self.touch(c.id)?;
        self.contract(&c.number)
    }

    fn touch(&self, id: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE contracts SET updated_at = ?2 WHERE id = ?1",
            params![id, now()],
        )?;
        Ok(())
    }

    /// Terms can change until the contract is executed.
    pub fn set_terms(&self, key: &str, t: &Terms) -> Result<Contract> {
        let c = self.contract(key)?;
        if !matches!(
            c.status,
            Status::Draft | Status::UnderReview | Status::PendingSignature
        ) {
            return Err(ContractError::State(format!(
                "{} is {}; terms are fixed after execution",
                c.number,
                c.status.as_str()
            )));
        }
        if t.start.is_some() != t.initial_months.is_some() {
            return Err(ContractError::Invalid(
                "start and initial_months go together".into(),
            ));
        }
        if t.initial_months.is_some_and(|m| !(1..=1200).contains(&m))
            || !(0..=1200).contains(&t.renewal_months)
            || !(0..=120).contains(&t.notice_months)
        {
            return Err(ContractError::Invalid(
                "initial 1-1200 months, renewal 0-1200, notice 0-120".into(),
            ));
        }
        if t.renewal_months > 0 && t.start.is_none() {
            return Err(ContractError::Invalid(
                "auto-renewal needs start and initial_months".into(),
            ));
        }
        if t.annual_value_cents.is_some_and(|v| v < 0) {
            return Err(ContractError::Invalid(
                "annual value cannot be negative".into(),
            ));
        }
        self.conn.execute(
            "UPDATE contracts SET start_date = ?2, initial_months = ?3, renewal_months = ?4, notice_months = ?5, annual_value_cents = ?6, updated_at = ?7 WHERE id = ?1",
            params![c.id, t.start, t.initial_months, t.renewal_months, t.notice_months, t.annual_value_cents, now()],
        )?;
        self.contract(&c.number)
    }

    /// Move along the status machine. `executed` needs a seal of the current version.
    pub fn transition(&self, key: &str, to: Status, actor: &str, note: &str) -> Result<Contract> {
        let c = self.contract(key)?;
        if !c.status.can_move_to(to) {
            let allowed = c
                .status
                .next()
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            return Err(ContractError::State(format!(
                "{} cannot go from {} to {} (allowed: {})",
                c.number,
                c.status.as_str(),
                to.as_str(),
                if allowed.is_empty() { "none" } else { &allowed }
            )));
        }
        if to == Status::Executed
            && !self
                .seals(&c)?
                .iter()
                .any(|s| s.version == c.current_version)
        {
            return Err(ContractError::State(format!(
                "seal version {} of {} before executing it",
                c.current_version, c.number
            )));
        }
        if matches!(to, Status::Terminated | Status::Cancelled) && note.trim().is_empty() {
            return Err(ContractError::Invalid(format!(
                "{} needs a note (reason)",
                to.as_str()
            )));
        }
        let tx = self.conn.unchecked_transaction()?;
        let stamp = now();
        tx.execute(
            "UPDATE contracts SET status = ?2, updated_at = ?3 WHERE id = ?1",
            params![c.id, to.as_str(), stamp],
        )?;
        tx.execute(
            "INSERT INTO transitions (contract_id, from_status, to_status, actor, note, at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![c.id, c.status.as_str(), to.as_str(), actor, note.trim(), stamp],
        )?;
        tx.commit()?;
        self.contract(&c.number)
    }

    pub fn history(&self, c: &Contract) -> Result<Vec<Transition>> {
        let mut st = self.conn.prepare("SELECT from_status, to_status, actor, note, at FROM transitions WHERE contract_id = ?1 ORDER BY id")?;
        let rows = st
            .query_map(params![c.id], |r| {
                Ok(Transition {
                    from: r.get(0)?,
                    to: r.get(1)?,
                    actor: r.get(2)?,
                    note: r.get(3)?,
                    at: r.get(4)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    // ---------- seals ----------

    pub fn add_seal(
        &self,
        c: &Contract,
        version: i64,
        sha256: &str,
        sealed_at: &str,
        signature: &str,
        public_key: &str,
    ) -> Result<Seal> {
        self.conn.execute(
            "INSERT INTO seals (contract_id, version, sha256, sealed_at, signature, public_key) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![c.id, version, sha256, sealed_at, signature, public_key],
        )?;
        let id = self.conn.last_insert_rowid();
        Ok(Seal {
            id,
            version,
            sha256: sha256.into(),
            sealed_at: sealed_at.into(),
            signature: signature.into(),
            public_key: public_key.into(),
        })
    }

    pub fn seals(&self, c: &Contract) -> Result<Vec<Seal>> {
        let mut st = self.conn.prepare("SELECT id, version, sha256, sealed_at, signature, public_key FROM seals WHERE contract_id = ?1 ORDER BY id")?;
        let rows = st
            .query_map(params![c.id], |r| {
                Ok(Seal {
                    id: r.get(0)?,
                    version: r.get(1)?,
                    sha256: r.get(2)?,
                    sealed_at: r.get(3)?,
                    signature: r.get(4)?,
                    public_key: r.get(5)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    // ---------- emitted events ----------

    pub fn emitted(&self, c: &Contract, kind: &str, key: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT event_id FROM emitted WHERE contract_id = ?1 AND kind = ?2 AND key = ?3",
                params![c.id, kind, key],
                |r| r.get(0),
            )
            .optional()?)
    }

    pub fn record_emitted(
        &self,
        c: &Contract,
        kind: &str,
        key: &str,
        event_id: &str,
    ) -> Result<()> {
        self.conn.execute("INSERT INTO emitted (contract_id, kind, key, event_id, at) VALUES (?1, ?2, ?3, ?4, ?5)", params![c.id, kind, key, event_id, now()])?;
        Ok(())
    }

    pub fn stats(&self) -> Result<Stats> {
        let mut by = BTreeMap::new();
        let mut st = self
            .conn
            .prepare("SELECT status, COUNT(*) FROM contracts GROUP BY status")?;
        for row in st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))? {
            let (k, v) = row?;
            by.insert(k, v);
        }
        Ok(Stats {
            templates: self.conn.query_row(
                "SELECT COUNT(DISTINCT name) FROM templates",
                [],
                |r| r.get(0),
            )?,
            contracts: by,
            sealed: self.conn.query_row(
                "SELECT COUNT(DISTINCT contract_id) FROM seals",
                [],
                |r| r.get(0),
            )?,
            demo: self.is_demo()?,
        })
    }
}

/// Distinct `{{name}}` placeholders in order of first use. Unclosed braces are an error.
pub fn placeholders(body: &str) -> Result<Vec<String>> {
    let mut out: Vec<String> = Vec::new();
    let mut rest = body;
    while let Some(i) = rest.find("{{") {
        let after = &rest[i + 2..];
        let j = after
            .find("}}")
            .ok_or_else(|| ContractError::Invalid("unclosed {{ in text".into()))?;
        let name = after[..j].trim();
        if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Err(ContractError::Invalid(format!(
                "bad placeholder {{{{{name}}}}}"
            )));
        }
        if !out.iter().any(|o| o == name) {
            out.push(name.to_string());
        }
        rest = &after[j + 2..];
    }
    Ok(out)
}

/// Fill a template. Unknown values, missing required values and leftovers are errors.
pub fn fill(
    t: &Template,
    values: &BTreeMap<String, String>,
) -> Result<(String, BTreeMap<String, String>)> {
    if let Some(k) = values
        .keys()
        .find(|k| !t.variables.iter().any(|v| &v.name == *k))
    {
        return Err(ContractError::Invalid(format!(
            "template {} has no variable {k}",
            t.name
        )));
    }
    let mut used = BTreeMap::new();
    let mut body = t.body.clone();
    for v in &t.variables {
        let val = match values
            .get(&v.name)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
        {
            Some(x) => x,
            None => match (&v.default, v.required) {
                (Some(d), _) => d.clone(),
                (None, true) => {
                    return Err(ContractError::Invalid(format!(
                        "missing required value {} ({})",
                        v.name, v.description
                    )))
                }
                (None, false) => String::new(),
            },
        };
        if val.contains("{{") {
            return Err(ContractError::Invalid(format!(
                "value of {} contains '{{{{'",
                v.name
            )));
        }
        body = body.replace(&format!("{{{{{}}}}}", v.name), &val);
        used.insert(v.name.clone(), val);
    }
    if !placeholders(&body)?.is_empty() {
        return Err(ContractError::Invalid(
            "unfilled placeholders remain".into(),
        ));
    }
    Ok((body, used))
}

#[cfg(test)]
mod tests;
