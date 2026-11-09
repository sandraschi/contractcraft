use super::*;

struct Env {
    ctx: Ctx,
    dir: PathBuf,
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn env(tag: &str) -> Env {
    let dir = std::env::temp_dir().join(format!("contractcraft-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let ctx = Ctx {
        db: Contracts::open_in_memory().unwrap(),
        confinement: Confinement::roots(std::slice::from_ref(&dir)).unwrap(),
        bus: Bus::new(dir.join("bus")),
        key_path: dir.join("seal.key"),
        actor: "test".into(),
    };
    Env { ctx, dir }
}

fn call(c: &mut Ctx, name: &str, a: Value) -> Result<Value, String> {
    let cmd = Command::from_tool(name, &a)?;
    dispatch(c, cmd)
}

fn d(y: i32, m: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, day).unwrap()
}

#[test]
fn catalog_tools_are_routable() {
    assert_eq!(tools().len(), 17);
    for t in tools() {
        let name = t["name"].as_str().unwrap();
        if let Err(err) = Command::from_tool(name, &json!({})) {
            assert!(!err.starts_with("unknown tool"), "{name}");
        }
    }
}

#[test]
fn demo_lifecycle_seal_and_verify() {
    let mut env = env("demo");
    let c = &mut env.ctx;
    call(c, "contract_demo_seed", json!({})).unwrap();
    assert!(call(c, "contract_demo_seed", json!({})).is_err());
    assert!(c.key_path.exists(), "seal key created on first seal");
    let one = call(
        c,
        "contract_show",
        json!({ "contract": "C-2026-001", "as_of": "2026-11-09" }),
    )
    .unwrap();
    assert_eq!(one["status"], "executed");
    assert_eq!(one["deadline"]["kind"], "notice");
    assert_eq!(one["deadline"]["due"], "2026-11-30");
    assert_eq!(one["deadline"]["term_end"], "2027-02-28");
    assert!(
        one["text"]
            .as_str()
            .unwrap()
            .contains("Gerichtsstand ist Wien."),
        "default filled"
    );
    let v = call(c, "contract_verify", json!({ "contract": "C-2026-001" })).unwrap();
    assert_eq!(v["ok"], true);
    // md export is byte-identical to the sealed text, a tampered copy is not.
    let md = c
        .confinement
        .check(&env.dir.join("c1.md").display().to_string(), true)
        .unwrap();
    let r = call(
        c,
        "contract_render",
        json!({ "contract": "C-2026-001", "format": "md", "path": md.display().to_string() }),
    )
    .unwrap();
    assert_eq!(r["matches_seal"], true);
    assert_eq!(
        call(
            c,
            "contract_verify",
            json!({ "contract": "C-2026-001", "path": md.display().to_string() })
        )
        .unwrap()["file"]["matches_seal"],
        true
    );
    let mut text = std::fs::read_to_string(&md).unwrap();
    text = text.replace("800,00 EUR", "8.000,00 EUR");
    std::fs::write(&md, text).unwrap();
    let bad = call(
        c,
        "contract_verify",
        json!({ "contract": "C-2026-001", "path": md.display().to_string() }),
    )
    .unwrap();
    assert_eq!(
        (bad["ok"].as_bool(), bad["file"]["matches_seal"].as_bool()),
        (Some(false), Some(false))
    );
    // Typst source works without the CLI.
    let typ = env.dir.join("c1.typ");
    call(
        c,
        "contract_render",
        json!({ "contract": "C-2026-001", "format": "typ", "path": typ.display().to_string() }),
    )
    .unwrap();
    assert!(std::fs::read_to_string(&typ)
        .unwrap()
        .contains("= Wartungsvertrag"));
}

#[test]
fn risk_scan_and_redline_delta_on_demo() {
    let mut env = env("risk");
    let c = &mut env.ctx;
    call(c, "contract_demo_seed", json!({})).unwrap();
    let nda = call(c, "contract_risk_scan", json!({ "contract": "C-2026-002" })).unwrap();
    let rules: Vec<&str> = nda["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["rule"].as_str().unwrap())
        .collect();
    for r in [
        "indemnity",
        "missing_dpa",
        "auto_renewal",
        "foreign_law",
        "foreign_jurisdiction",
    ] {
        assert!(rules.contains(&r), "{r} in {rules:?}");
    }
    let rl = call(c, "contract_redline", json!({ "contract": "C-2026-003" })).unwrap();
    assert_eq!(
        (
            rl["stats"]["lines_inserted"].as_u64(),
            rl["stats"]["lines_deleted"].as_u64()
        ),
        (Some(1), Some(1))
    );
    assert_eq!(rl["stats"]["risk_added"], json!(["unlimited_liability"]));
    assert!(rl["unified"]
        .as_str()
        .unwrap()
        .contains("+Der Auftragnehmer haftet unbeschränkt"));
    // Text is frozen outside draft.
    assert!(call(
        c,
        "contract_edit",
        json!({ "contract": "C-2026-003", "body": "x" })
    )
    .unwrap_err()
    .contains("draft"));
}

#[test]
fn draft_validation() {
    let mut env = env("draft");
    let c = &mut env.ctx;
    call(c, "contract_demo_seed", json!({})).unwrap();
    let missing = call(
        c,
        "contract_draft",
        json!({ "template": "NDA (DEMO)", "title": "x", "counterparty": "y", "values": { "party": "A" } }),
    );
    assert!(missing.unwrap_err().contains("purpose"));
    let typo = call(
        c,
        "contract_draft",
        json!({ "template": "NDA (DEMO)", "title": "x", "counterparty": "y", "values": { "party": "A", "purpose": "B", "venue": "Wien", "venu": "x" } }),
    );
    assert!(typo.unwrap_err().contains("venu"));
    assert!(call(
        c,
        "contract_draft",
        json!({ "title": "x", "counterparty": "y" })
    )
    .is_err());
    let own = call(c, "contract_draft", json!({ "title": "Kaufvertrag", "counterparty": "Anna Mockova (DEMO)", "body": "Kauf eines Laptops. Gerichtsstand Wien." })).unwrap();
    assert_eq!(own["risk_findings"], 0);
    assert!(call(
        c,
        "contract_terms",
        json!({ "contract": own["number"], "annual_value": 1200.5 })
    )
    .unwrap_err()
    .contains("no floats"));
}

#[test]
fn deadlines_emit_once_and_executed_event() {
    let mut env = env("dl");
    let c = &mut env.ctx;
    call(c, "contract_demo_seed", json!({})).unwrap();
    let out = deadlines(c, 60, d(2026, 11, 9), true).unwrap();
    assert_eq!(out["emitted"], 1);
    assert_eq!(out["rows"][0]["due"], "2026-11-30");
    assert_eq!(
        deadlines(c, 60, d(2026, 11, 9), true).unwrap()["emitted"],
        0,
        "once per deadline"
    );
    let ev = c.bus.pending("taskcraft").unwrap();
    assert_eq!(ev.len(), 1);
    assert_eq!(ev[0].kind, "contract.deadline");
    assert_eq!(ev[0].payload["due_date"], "2026-11-30");
    assert!(ev[0].payload["title"]
        .as_str()
        .unwrap()
        .starts_with("Kündigungsfrist"));
    // A fixed-term contract executed now emits contract.executed to gaincraft once.
    let n = call(c, "contract_draft", json!({ "title": "Projekt", "counterparty": "Mia Mockerl (DEMO)", "body": "Projektvertrag, Gerichtsstand Wien." })).unwrap()["number"].as_str().unwrap().to_string();
    call(c, "contract_terms", json!({ "contract": n, "start": "2026-12-01", "initial_months": 6, "annual_value": "12.000,00" })).unwrap();
    for to in ["under_review", "pending_signature"] {
        call(c, "contract_transition", json!({ "contract": n, "to": to })).unwrap();
    }
    assert!(call(
        c,
        "contract_transition",
        json!({ "contract": n, "to": "executed" })
    )
    .unwrap_err()
    .contains("seal"));
    call(c, "contract_seal", json!({ "contract": n })).unwrap();
    assert!(
        call(c, "contract_seal", json!({ "contract": n })).is_err(),
        "already sealed"
    );
    let done = call(
        c,
        "contract_transition",
        json!({ "contract": n, "to": "executed" }),
    )
    .unwrap();
    assert!(done["emitted"]["gaincraft"].is_string());
    let g = c.bus.pending("gaincraft").unwrap();
    assert_eq!(
        (g.len(), g[0].payload["annual_value_cents"].as_i64()),
        (1, Some(1_200_000))
    );
    assert_eq!(g[0].payload["term_end"], "2027-05-31");
    let fixed = deadlines(c, 400, d(2026, 11, 9), false).unwrap();
    assert!(fixed["rows"]
        .as_array()
        .unwrap()
        .iter()
        .any(|r| r["kind"] == "expiry" && r["due"] == "2027-05-31"));
}

#[test]
fn pdf_without_typst_fails_cleanly() {
    let mut env = env("pdf");
    let c = &mut env.ctx;
    call(c, "contract_demo_seed", json!({})).unwrap();
    std::env::set_var("CONTRACTCRAFT_TYPST", "D:/nope/typst-missing.exe");
    let out = env.dir.join("c1.pdf");
    let err = call(
        c,
        "contract_render",
        json!({ "contract": "C-2026-001", "format": "pdf", "path": out.display().to_string() }),
    )
    .unwrap_err();
    std::env::remove_var("CONTRACTCRAFT_TYPST");
    assert!(err.contains("Typst CLI"));
    assert!(!out.exists(), "no fake PDF");
    assert!(
        call(
            c,
            "contract_render",
            json!({ "contract": "C-2026-001", "format": "md", "path": "C:/Windows/c.md" })
        )
        .is_err(),
        "confined"
    );
}
