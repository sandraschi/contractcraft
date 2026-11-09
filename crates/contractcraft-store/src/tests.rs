use super::*;

fn d(y: i32, m: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, day).unwrap()
}

fn var(name: &str, required: bool, default: Option<&str>) -> Variable {
    Variable {
        name: name.into(),
        required,
        default: default.map(str::to_string),
        description: String::new(),
    }
}

fn new_contract(c: &Contracts) -> Contract {
    c.create(
        &NewContract {
            title: "Wartung".into(),
            counterparty: "Mock GmbH".into(),
            body: "Text v1".into(),
            ..Default::default()
        },
        "test",
        d(2026, 11, 9),
    )
    .unwrap()
}

#[test]
fn template_placeholders_must_match_declarations() {
    let c = Contracts::open_in_memory().unwrap();
    let vars = vec![var("kunde", true, None), var("ort", false, Some("Wien"))];
    let t = c
        .add_template(
            "Dienstvertrag",
            "",
            &vars,
            "Zwischen {{kunde}} und uns. Gerichtsstand {{ort}}. {{kunde}}",
        )
        .unwrap();
    assert_eq!(t.version, 1);
    assert!(c
        .add_template("X", "", &vars, "nur {{kunde}}")
        .unwrap_err()
        .to_string()
        .contains("never used"));
    assert!(c
        .add_template("X", "", &vars[..1], "{{kunde}} {{ort}}")
        .unwrap_err()
        .to_string()
        .contains("not declared"));
    assert!(
        c.add_template("X", "", &vars, "{{kunde}} {{ort").is_err(),
        "unclosed"
    );
    let t2 = c
        .add_template("Dienstvertrag", "v2", &vars, "Neu: {{kunde}} in {{ort}}")
        .unwrap();
    assert_eq!(t2.version, 2);
    assert_eq!(c.template("Dienstvertrag", Some(1)).unwrap().version, 1);
    assert_eq!(c.templates().unwrap().len(), 1);
}

#[test]
fn fill_checks_values() {
    let t = Template {
        name: "T".into(),
        version: 1,
        description: String::new(),
        variables: vec![
            var("kunde", true, None),
            var("ort", false, Some("Wien")),
            var("extra", false, None),
        ],
        body: "{{kunde}} / {{ort}} / [{{extra}}]".into(),
        created_at: String::new(),
    };
    let mut v = BTreeMap::new();
    assert!(fill(&t, &v).unwrap_err().to_string().contains("kunde"));
    v.insert("kunde".into(), "Joe Mocky e.U.".into());
    assert_eq!(fill(&t, &v).unwrap().0, "Joe Mocky e.U. / Wien / []");
    v.insert("kundee".into(), "typo".into());
    assert!(fill(&t, &v).unwrap_err().to_string().contains("kundee"));
    v.remove("kundee");
    v.insert("ort".into(), "{{kunde}}".into());
    assert!(fill(&t, &v).is_err(), "no placeholder injection");
}

#[test]
fn numbering_and_status_machine() {
    let c = Contracts::open_in_memory().unwrap();
    let a = new_contract(&c);
    let b = new_contract(&c);
    assert_eq!(
        (a.number.as_str(), b.number.as_str()),
        ("C-2026-001", "C-2026-002")
    );
    assert!(c
        .transition(&a.number, Status::Executed, "t", "")
        .unwrap_err()
        .to_string()
        .contains("allowed: under_review, cancelled"));
    c.transition(&a.number, Status::UnderReview, "t", "")
        .unwrap();
    c.transition(&a.number, Status::PendingSignature, "t", "")
        .unwrap();
    assert!(
        c.transition(&a.number, Status::Executed, "t", "")
            .unwrap_err()
            .to_string()
            .contains("seal"),
        "unsealed"
    );
    let cur = c.contract(&a.number).unwrap();
    c.add_seal(&cur, 1, "ab", "2026-11-09T10:00:00Z", "sig", "pk")
        .unwrap();
    c.transition(&a.number, Status::Executed, "t", "").unwrap();
    assert!(
        c.transition(&a.number, Status::Terminated, "t", " ")
            .is_err(),
        "needs a reason"
    );
    c.transition(&a.number, Status::Terminated, "t", "Kündigung zum 31.12.")
        .unwrap();
    assert!(Status::Terminated.next().is_empty());
    assert_eq!(c.history(&c.contract(&a.number).unwrap()).unwrap().len(), 5);
}

#[test]
fn text_changes_only_in_draft_and_history_is_immutable() {
    let c = Contracts::open_in_memory().unwrap();
    let a = new_contract(&c);
    assert!(c.edit(&a.number, "Text v1", "").is_err(), "unchanged");
    let e = c.edit(&a.number, "Text v2", "Haftung begrenzt").unwrap();
    assert_eq!(e.current_version, 2);
    c.transition(&a.number, Status::UnderReview, "t", "")
        .unwrap();
    assert!(matches!(
        c.edit(&a.number, "Text v3", ""),
        Err(ContractError::State(_))
    ));
    // Triggers hold when Rust is bypassed.
    assert!(c.conn().execute("INSERT INTO contract_versions (contract_id, version, body, created_at) VALUES (?1, 9, 'x', 'now')", params![a.id]).unwrap_err().to_string().contains("draft"));
    assert!(c
        .conn()
        .execute(
            "UPDATE contract_versions SET body = 'forged' WHERE contract_id = ?1",
            params![a.id]
        )
        .is_err());
    assert!(c
        .conn()
        .execute(
            "DELETE FROM contract_versions WHERE contract_id = ?1",
            params![a.id]
        )
        .is_err());
    let cur = c.contract(&a.number).unwrap();
    c.add_seal(&cur, 2, "h", "t", "s", "p").unwrap();
    assert!(c
        .conn()
        .execute("UPDATE seals SET sha256 = 'x'", [])
        .is_err());
    assert_eq!(c.version(&cur, Some(1)).unwrap().body, "Text v1");
}

#[test]
fn terms_validation_and_lock_after_execution() {
    let c = Contracts::open_in_memory().unwrap();
    let a = new_contract(&c);
    assert!(
        c.set_terms(
            &a.number,
            &Terms {
                start: Some(d(2026, 1, 1)),
                ..Default::default()
            }
        )
        .is_err(),
        "start without months"
    );
    assert!(
        c.set_terms(
            &a.number,
            &Terms {
                renewal_months: 12,
                ..Default::default()
            }
        )
        .is_err(),
        "renewal without start"
    );
    let t = Terms {
        start: Some(d(2026, 1, 1)),
        initial_months: Some(12),
        renewal_months: 12,
        notice_months: 3,
        annual_value_cents: Some(1_200_000),
    };
    assert_eq!(c.set_terms(&a.number, &t).unwrap().terms, t);
    for s in [Status::UnderReview, Status::PendingSignature] {
        c.transition(&a.number, s, "t", "").unwrap();
    }
    let cur = c.contract(&a.number).unwrap();
    c.add_seal(&cur, 1, "h", "t", "s", "p").unwrap();
    c.transition(&a.number, Status::Executed, "t", "").unwrap();
    assert!(matches!(
        c.set_terms(&a.number, &t),
        Err(ContractError::State(_))
    ));
}
