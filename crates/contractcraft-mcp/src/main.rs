//! `contractcraft`: CLI and MCP stdio server in one binary.
//!
//! Every CLI verb builds the same tool-argument JSON an MCP client would send
//! and goes through `Command::from_tool` + `dispatch`, so CLI and MCP share one
//! code path. In `mcp` mode stdout carries only JSON-RPC; logs go to stderr.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use clap::{Parser, Subcommand};
use contractcraft_core::{dispatch, tools, Command, Confinement, Ctx, APP, VERSION};
use contractcraft_store::Contracts;
use craft_biz::bus::Bus;
use craft_biz::mcp::{serve_stdio, ToolHost};
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const AFTER_HELP: &str = "Typical contract:
  contractcraft draft \"Wartung\" \"Mock GmbH\" --template \"Wartungsvertrag (DEMO)\" --set kunde=\"Mock GmbH\" ...
  contractcraft scan C-2026-004                # risk clauses, not legal advice
  contractcraft move C-2026-004 under_review; contractcraft redline C-2026-004
  contractcraft move C-2026-004 pending_signature; contractcraft seal C-2026-004
  contractcraft move C-2026-004 executed
  contractcraft deadlines --days 90 --emit     # notice deadlines to taskcraft
See HELP.md for workflows and docs/TOOLS.md for every MCP tool.";

#[derive(Parser)]
#[command(after_help = AFTER_HELP,
    name = "contractcraft",
    version,
    about = "Contract templates, review, redlines, risk scan, seals and notice deadlines. CLI + MCP."
)]
struct Cli {
    /// Database. Default: $CONTRACTCRAFT_DB or %LOCALAPPDATA%\contractcraft\contractcraft.db
    #[arg(long, global = true)]
    db: Option<PathBuf>,
    /// Seal key file. Default: seal.key next to the database.
    #[arg(long, global = true)]
    key: Option<PathBuf>,
    /// Print machine-readable JSON only.
    #[arg(long, global = true)]
    json: bool,
    /// Restrict file access to these directories (repeatable). Required for file tools in `mcp` mode.
    #[arg(long = "allow-path", global = true)]
    allow_path: Vec<PathBuf>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Counts by status.
    Status,
    /// Templates.
    Template {
        #[command(subcommand)]
        op: TemplateOp,
    },
    /// New draft from a template (--set key=value, repeatable) or from a text file (--file).
    Draft {
        title: String,
        counterparty: String,
        #[arg(long)]
        template: Option<String>,
        #[arg(long = "set")]
        set: Vec<String>,
        #[arg(long)]
        file: Option<String>,
    },
    /// List contracts.
    List {
        #[arg(long)]
        status: Option<String>,
        #[arg(long)]
        text: Option<String>,
    },
    /// Show a contract (text, history, seals, risk, deadline).
    Show {
        contract: String,
        #[arg(long)]
        version: Option<i64>,
    },
    /// New text version from a file (draft only).
    Edit {
        contract: String,
        file: String,
        #[arg(long)]
        note: Option<String>,
    },
    /// Term data for deadlines.
    Terms {
        contract: String,
        #[arg(long)]
        start: Option<String>,
        #[arg(long)]
        months: Option<i64>,
        #[arg(long, default_value_t = 0)]
        renew: i64,
        #[arg(long, default_value_t = 0)]
        notice: i64,
        /// Annual value, DE format ("9.600,00")
        #[arg(long)]
        value: Option<String>,
    },
    /// Move to another status.
    Move {
        contract: String,
        to: String,
        #[arg(long)]
        note: Option<String>,
    },
    /// Seal the current version.
    Seal { contract: String },
    /// Verify seals (and optionally a file).
    Verify {
        contract: String,
        #[arg(long)]
        file: Option<String>,
    },
    /// Diff two versions.
    Redline {
        contract: String,
        #[arg(long)]
        from: Option<i64>,
        #[arg(long)]
        to: Option<i64>,
    },
    /// Clause risk scan of a contract or a text file.
    Scan {
        contract: Option<String>,
        #[arg(long)]
        file: Option<String>,
        #[arg(long)]
        rules: Option<String>,
    },
    /// Write md, typ or pdf.
    Render {
        contract: String,
        path: String,
        #[arg(long, default_value = "md")]
        format: String,
        #[arg(long)]
        version: Option<i64>,
    },
    /// Notice deadlines within N days; --emit sends them to taskcraft.
    Deadlines {
        #[arg(long, default_value_t = 90)]
        days: i64,
        #[arg(long)]
        emit: bool,
    },
    /// Open draft contracts from hirecraft candidate.hired events.
    DrainInbox,
    /// Fill an empty database with labelled demo data.
    DemoSeed,
    /// Print the MCP tool catalog (JSON, or markdown for docs/TOOLS.md).
    Tools {
        #[arg(long)]
        markdown: bool,
    },
    /// Run the MCP stdio server.
    Mcp,
}

#[derive(Subcommand)]
enum TemplateOp {
    List,
    Show {
        name: String,
        #[arg(long)]
        version: Option<i64>,
    },
    /// Add from a text file; variables as name, name! (required) or name=default.
    Add {
        name: String,
        file: String,
        #[arg(long = "var")]
        vars: Vec<String>,
        #[arg(long)]
        description: Option<String>,
    },
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

fn default_key(db: &Path) -> PathBuf {
    db.parent()
        .map(|p| p.join("seal.key"))
        .unwrap_or_else(|| PathBuf::from("seal.key"))
}

fn obj(pairs: Vec<(&str, Option<Value>)>) -> Value {
    let mut m = Map::new();
    for (k, v) in pairs {
        if let Some(v) = v {
            m.insert(k.to_string(), v);
        }
    }
    Value::Object(m)
}
fn sv(s: &str) -> Option<Value> {
    Some(Value::String(s.to_string()))
}
fn os(s: Option<String>) -> Option<Value> {
    s.map(Value::String)
}

/// `name`, `name!` (required) or `name=default`.
fn parse_var(spec: &str) -> Value {
    if let Some((n, d)) = spec.split_once('=') {
        json!({ "name": n.trim(), "default": d })
    } else if let Some(n) = spec.strip_suffix('!') {
        json!({ "name": n.trim(), "required": true })
    } else {
        json!({ "name": spec.trim() })
    }
}

fn to_tool(cmd: Cmd) -> Result<(&'static str, Value), String> {
    Ok(match cmd {
        Cmd::Status => ("contract_status", json!({})),
        Cmd::Template {
            op: TemplateOp::List,
        } => ("contract_template_list", json!({})),
        Cmd::Template {
            op: TemplateOp::Show { name, version },
        } => (
            "contract_template_show",
            obj(vec![
                ("name", sv(&name)),
                ("version", version.map(|v| json!(v))),
            ]),
        ),
        Cmd::Template {
            op:
                TemplateOp::Add {
                    name,
                    file,
                    vars,
                    description,
                },
        } => (
            "contract_template_add",
            obj(vec![
                ("name", sv(&name)),
                ("body_path", sv(&file)),
                (
                    "variables",
                    Some(Value::Array(vars.iter().map(|v| parse_var(v)).collect())),
                ),
                ("description", os(description)),
            ]),
        ),
        Cmd::Draft {
            title,
            counterparty,
            template,
            set,
            file,
        } => {
            let mut values = Map::new();
            for kv in set {
                let (k, v) = kv
                    .split_once('=')
                    .ok_or_else(|| format!("--set {kv:?}: use key=value"))?;
                values.insert(k.trim().to_string(), Value::String(v.to_string()));
            }
            (
                "contract_draft",
                obj(vec![
                    ("title", sv(&title)),
                    ("counterparty", sv(&counterparty)),
                    ("template", os(template)),
                    (
                        "values",
                        (!values.is_empty()).then_some(Value::Object(values)),
                    ),
                    ("body_path", os(file)),
                ]),
            )
        }
        Cmd::List { status, text } => (
            "contract_list",
            obj(vec![("status", os(status)), ("text", os(text))]),
        ),
        Cmd::Show { contract, version } => (
            "contract_show",
            obj(vec![
                ("contract", sv(&contract)),
                ("version", version.map(|v| json!(v))),
            ]),
        ),
        Cmd::Edit {
            contract,
            file,
            note,
        } => (
            "contract_edit",
            obj(vec![
                ("contract", sv(&contract)),
                ("body_path", sv(&file)),
                ("note", os(note)),
            ]),
        ),
        Cmd::Terms {
            contract,
            start,
            months,
            renew,
            notice,
            value,
        } => (
            "contract_terms",
            obj(vec![
                ("contract", sv(&contract)),
                ("start", os(start)),
                ("initial_months", months.map(|m| json!(m))),
                ("renewal_months", Some(json!(renew))),
                ("notice_months", Some(json!(notice))),
                ("annual_value", os(value)),
            ]),
        ),
        Cmd::Move { contract, to, note } => (
            "contract_transition",
            obj(vec![
                ("contract", sv(&contract)),
                ("to", sv(&to)),
                ("note", os(note)),
            ]),
        ),
        Cmd::Seal { contract } => ("contract_seal", json!({ "contract": contract })),
        Cmd::Verify { contract, file } => (
            "contract_verify",
            obj(vec![("contract", sv(&contract)), ("path", os(file))]),
        ),
        Cmd::Redline { contract, from, to } => (
            "contract_redline",
            obj(vec![
                ("contract", sv(&contract)),
                ("from", from.map(|v| json!(v))),
                ("to", to.map(|v| json!(v))),
            ]),
        ),
        Cmd::Scan {
            contract,
            file,
            rules,
        } => {
            let text = match file {
                Some(f) => Some(std::fs::read_to_string(&f).map_err(|e| format!("{f}: {e}"))?),
                None => None,
            };
            (
                "contract_risk_scan",
                obj(vec![
                    ("contract", os(contract)),
                    ("text", os(text)),
                    ("rules_path", os(rules)),
                ]),
            )
        }
        Cmd::Render {
            contract,
            path,
            format,
            version,
        } => (
            "contract_render",
            obj(vec![
                ("contract", sv(&contract)),
                ("path", sv(&path)),
                ("format", sv(&format)),
                ("version", version.map(|v| json!(v))),
            ]),
        ),
        Cmd::Deadlines { days, emit } => (
            "contract_deadlines",
            json!({ "within_days": days, "emit": emit }),
        ),
        Cmd::DrainInbox => ("contract_drain_inbox", json!({})),
        Cmd::DemoSeed => ("contract_demo_seed", json!({})),
        Cmd::Tools { .. } | Cmd::Mcp => return Err("not a tool verb".into()),
    })
}

struct Host {
    ctx: Ctx,
}

impl ToolHost for Host {
    fn server_name(&self) -> &str {
        APP
    }
    fn server_version(&self) -> &str {
        VERSION
    }
    fn tools(&self) -> Vec<Value> {
        tools()
    }
    fn call(&mut self, name: &str, args: &Value) -> Result<Value, String> {
        let cmd = Command::from_tool(name, args)?;
        dispatch(&mut self.ctx, cmd)
    }
}

fn scalar(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => "-".into(),
        Value::Array(a) => a.iter().map(scalar).collect::<Vec<_>>().join(","),
        other => other.to_string(),
    }
}

const COLUMN_ORDER: &[&str] = &[
    "number",
    "contract",
    "severity",
    "rule",
    "op",
    "line",
    "title",
    "counterparty",
    "status",
    "kind",
    "due",
    "days_left",
    "term_end",
    "paragraph",
    "excerpt",
    "version",
    "sealed_at",
    "ok",
    "hash_matches",
    "signature_valid",
    "current_version",
    "annual_value",
    "next",
    "name",
    "variables",
    "description",
    "sent_to_taskcraft",
];
const HIDDEN: &[&str] = &[
    "id",
    "values",
    "terms",
    "created_at",
    "updated_at",
    "template_version",
    "advice",
    "deadline",
    "template",
];

fn columns(rows: &[Value]) -> Vec<String> {
    let mut keys: Vec<String> = Vec::new();
    for r in rows.iter().filter_map(Value::as_object) {
        for (k, v) in r {
            if !v.is_object() && !HIDDEN.contains(&k.as_str()) && !keys.contains(k) {
                keys.push(k.clone());
            }
        }
    }
    keys.sort_by_key(|k| {
        (
            COLUMN_ORDER
                .iter()
                .position(|c| c == k)
                .unwrap_or(usize::MAX),
            k.clone(),
        )
    });
    keys
}

/// Human rendering: show prints the text; lists become aligned tables.
fn render(v: &Value) -> String {
    let Some(obj) = v.as_object() else {
        return scalar(v);
    };
    if let (Some(text), Some(number)) = (
        obj.get("text").and_then(Value::as_str),
        obj.get("number").and_then(Value::as_str),
    ) {
        let mut out = format!(
            "{number}  {}  ({})\nstatus: {}  version: {}\n",
            scalar(&obj["title"]),
            scalar(&obj["counterparty"]),
            scalar(&obj["status"]),
            scalar(&obj["text_version"])
        );
        if let Some(d) = obj.get("deadline").filter(|d| !d.is_null()) {
            out.push_str(&format!(
                "next deadline: {} {} ({} days)\n",
                scalar(&d["kind"]),
                scalar(&d["due"]),
                scalar(&d["days_left"])
            ));
        }
        let risks = obj
            .get("risk")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or(0);
        out.push_str(&format!(
            "seals: {}  risk findings: {risks}\n\n{text}\n",
            obj.get("seals")
                .and_then(Value::as_array)
                .map(Vec::len)
                .unwrap_or(0)
        ));
        return out;
    }
    if let Some(u) = obj.get("unified").and_then(Value::as_str) {
        return format!(
            "{}\nrisk added: {}  risk removed: {}\n",
            u.trim_end(),
            scalar(&obj["stats"]["risk_added"]),
            scalar(&obj["stats"]["risk_removed"])
        );
    }
    const LISTS: [&str; 3] = ["rows", "contracts", "templates"];
    let Some(key) = LISTS
        .iter()
        .find(|k| obj.get(**k).is_some_and(Value::is_array))
    else {
        return serde_json::to_string_pretty(v).unwrap_or_else(|e| e.to_string());
    };
    let mut out = String::new();
    for (k, val) in obj {
        if !val.is_array() && !val.is_object() {
            out.push_str(&format!("{k}: {}\n", scalar(val)));
        }
    }
    let rows = obj
        .get(*key)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if rows.is_empty() {
        out.push_str("(none)\n");
        return out;
    }
    let cols = columns(&rows);
    let table: Vec<Vec<String>> = rows
        .iter()
        .map(|r| {
            cols.iter()
                .map(|c| r.get(c).map(scalar).unwrap_or_else(|| "-".into()))
                .collect()
        })
        .collect();
    let widths: Vec<usize> = cols
        .iter()
        .enumerate()
        .map(|(i, c)| {
            table
                .iter()
                .map(|r| r[i].chars().count().min(70))
                .chain([c.chars().count()])
                .max()
                .unwrap_or(0)
        })
        .collect();
    let line = |cells: &[String]| -> String {
        let padded: Vec<String> = cells
            .iter()
            .zip(&widths)
            .map(|(c, w)| {
                let c: String = if c.chars().count() > 70 {
                    format!("{}...", c.chars().take(67).collect::<String>())
                } else {
                    c.clone()
                };
                format!("{c}{}", " ".repeat(w.saturating_sub(c.chars().count())))
            })
            .collect();
        format!("{}\n", padded.join("  ").trim_end())
    };
    out.push_str(&line(&cols));
    for r in &table {
        out.push_str(&line(r));
    }
    out
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    if let Cmd::Tools { markdown } = cli.cmd {
        if markdown {
            print!("{}", craft_biz::mcp::tools_markdown(APP, &tools()));
            return ExitCode::SUCCESS;
        }
        println!(
            "{}",
            serde_json::to_string_pretty(&tools()).unwrap_or_default()
        );
        return ExitCode::SUCCESS;
    }
    let db_path = cli.db.clone().unwrap_or_else(default_db);
    let db = match Contracts::open(&db_path) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("contractcraft: cannot open {}: {e}", db_path.display());
            return ExitCode::FAILURE;
        }
    };
    let is_mcp = matches!(cli.cmd, Cmd::Mcp);
    let confinement = if is_mcp || !cli.allow_path.is_empty() {
        match Confinement::roots(&cli.allow_path) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("contractcraft: {e}");
                return ExitCode::FAILURE;
            }
        }
    } else {
        Confinement(None)
    };
    let actor = if is_mcp {
        "agent".to_string()
    } else {
        std::env::var("USERNAME")
            .or_else(|_| std::env::var("USER"))
            .unwrap_or_else(|_| "cli".into())
    };
    let key_path = cli.key.clone().unwrap_or_else(|| default_key(&db_path));
    let ctx = Ctx {
        db,
        confinement,
        bus: Bus::new(Bus::default_root()),
        key_path,
        actor,
    };
    if is_mcp {
        eprintln!("[contractcraft] db {}", db_path.display());
        let mut host = Host { ctx };
        return match serve_stdio(&mut host) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("contractcraft mcp: {e}");
                ExitCode::FAILURE
            }
        };
    }
    let json_out = cli.json;
    let mut ctx = ctx;
    let result = to_tool(cli.cmd)
        .and_then(|(name, args)| Command::from_tool(name, &args))
        .and_then(|cmd| dispatch(&mut ctx, cmd));
    match result {
        Ok(v) => {
            if json_out {
                println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
            } else {
                print!("{}", render(&v));
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            if json_out {
                println!("{}", json!({ "error": e }));
            } else {
                eprintln!("contractcraft: {e}");
            }
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use craft_biz::mcp::handle_line;

    #[test]
    fn tools_doc_is_current() {
        let committed = include_str!("../../../docs/TOOLS.md");
        assert!(
            craft_biz::mcp::docs_in_sync(committed, &craft_biz::mcp::tools_markdown(APP, &tools())),
            "docs/TOOLS.md is stale: run `just docs`"
        );
    }

    fn host(tag: &str) -> Host {
        let dir =
            std::env::temp_dir().join(format!("contractcraft-mcp-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        Host {
            ctx: Ctx {
                db: Contracts::open_in_memory().unwrap(),
                confinement: Confinement(Some(Vec::new())),
                bus: Bus::new(dir.join("bus")),
                key_path: dir.join("seal.key"),
                actor: "agent".into(),
            },
        }
    }

    fn text(v: &Value) -> String {
        v["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or("")
            .to_string()
    }

    #[test]
    fn mcp_probe_initialize_list_call() {
        let mut h = host("probe");
        let init = handle_line(
            &mut h,
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
        )
        .unwrap();
        assert_eq!(init["result"]["serverInfo"]["name"], "contractcraft");
        let list =
            handle_line(&mut h, r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#).unwrap();
        assert_eq!(list["result"]["tools"].as_array().map(Vec::len), Some(17));
        let seed = handle_line(&mut h, r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"contract_demo_seed","arguments":{}}}"#).unwrap();
        assert_eq!(seed["result"]["isError"], false, "{seed}");
        let scan = handle_line(&mut h, r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"contract.risk_scan","arguments":{"text":"Der Auftragnehmer haftet unbeschränkt."}}}"#).unwrap();
        assert!(text(&scan).contains("unlimited_liability"));
        let mv = handle_line(&mut h, r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"contract_transition","arguments":{"contract":"C-2026-002","to":"executed"}}}"#).unwrap();
        assert_eq!(mv["result"]["isError"], true, "status machine");
        let render = handle_line(&mut h, r#"{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"contract_render","arguments":{"contract":"C-2026-001","path":"C:/Windows/x.md"}}}"#).unwrap();
        assert_eq!(render["result"]["isError"], true, "confined");
        let history = handle_line(&mut h, r#"{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"contract_show","arguments":{"contract":"C-2026-001"}}}"#).unwrap();
        let parsed: Value = serde_json::from_str(&text(&history)).unwrap();
        assert!(parsed["history"]
            .as_array()
            .unwrap()
            .iter()
            .all(|t| t["actor"] == "agent"));
        if let Some(dir) = h.ctx.key_path.parent() {
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    #[test]
    fn cli_verbs_map_to_known_tools() {
        let verbs = vec![
            Cmd::Status,
            Cmd::Template {
                op: TemplateOp::List,
            },
            Cmd::List {
                status: None,
                text: None,
            },
            Cmd::Seal {
                contract: "C-2026-001".into(),
            },
            Cmd::Redline {
                contract: "C-2026-003".into(),
                from: None,
                to: None,
            },
            Cmd::Deadlines {
                days: 60,
                emit: false,
            },
            Cmd::Draft {
                title: "t".into(),
                counterparty: "c".into(),
                template: Some("NDA".into()),
                set: vec!["party=A".into()],
                file: None,
            },
            Cmd::DemoSeed,
        ];
        let names: Vec<String> = tools()
            .iter()
            .filter_map(|t| t["name"].as_str().map(str::to_string))
            .collect();
        for v in verbs {
            let (name, _) = to_tool(v).unwrap();
            assert!(names.iter().any(|n| n == name), "{name}");
        }
        assert!(to_tool(Cmd::Draft {
            title: "t".into(),
            counterparty: "c".into(),
            template: None,
            set: vec!["novalue".into()],
            file: None
        })
        .is_err());
        assert_eq!(
            parse_var("kunde!"),
            json!({ "name": "kunde", "required": true })
        );
        assert_eq!(
            parse_var("ort=Wien"),
            json!({ "name": "ort", "default": "Wien" })
        );
    }

    #[test]
    fn render_show_and_redline() {
        let v = json!({ "number": "C-1", "title": "T", "counterparty": "X", "status": "draft", "text_version": 2, "text": "Hallo", "seals": [], "risk": [], "deadline": null });
        assert!(render(&v).contains("Hallo"));
        let r = json!({ "unified": "--- a\n+++ b\n", "stats": { "risk_added": ["x"], "risk_removed": [] } });
        assert!(render(&r).contains("risk added: x"));
    }
}
