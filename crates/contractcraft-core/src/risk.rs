//! Deterministic clause scan with a TOML rule file (DE + EN patterns).
//! Findings are pointers for a human reviewer, not legal advice.
//!
//! Rule kinds:
//! - `match`:  flag each paragraph matching `pattern`, unless `unless` also
//!   matches in that paragraph.
//! - `absent`: flag once when `when` matches somewhere and `pattern` matches
//!   nowhere in the text (e.g. personal data without a processing agreement).

use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};

pub const DEFAULT_RULES: &str = r#"
[[rule]]
id = "unlimited_liability"
title = "Unbeschränkte Haftung / unlimited liability"
severity = "high"
kind = "match"
pattern = 'unbeschränkt\w*\s+haft|haftet\s+(\w+\s+){0,3}unbeschränkt|unbegrenzt\w*\s+haftung|haftung\s+(\w+\s+){0,3}unbegrenzt|unlimited\s+liability|liability\s+(\w+\s+){0,3}unlimited|without\s+(any\s+)?limitation\s+of\s+liability'
advice = "Cap liability (e.g. annual contract value), exclude indirect damages, keep the statutory exceptions (intent, gross negligence, personal injury)."

[[rule]]
id = "indemnity"
title = "Schad- und Klagloshaltung / indemnity"
severity = "medium"
kind = "match"
pattern = 'schad-?\s*und\s+klaglos|klag-?\s*und\s+schadlos|indemnif\w+|hold\s+(us\s+|\w+\s+)?harmless'
advice = "Limit to third-party claims caused by your own breach, with notice and defence control, and under the liability cap."

[[rule]]
id = "auto_renewal"
title = "Automatische Verlängerung / automatic renewal"
severity = "medium"
kind = "match"
pattern = 'verlängert\s+sich\s+(\w+\s+){0,3}(automatisch|jeweils|stillschweigend)|automatisch\w*\s+verläng|stillschweigend\w*\s+verläng|renews?\s+automatically|automatically\s+renew|auto-?renew'
advice = "Put the notice deadline in the calendar (contract terms) and check the notice period."

[[rule]]
id = "foreign_jurisdiction"
title = "Gerichtsstand außerhalb Österreichs / foreign jurisdiction"
severity = "high"
kind = "match"
pattern = 'gerichtsstand|place\s+of\s+jurisdiction|jurisdiction|venue'
unless = 'wien|vienna|österreich|austria|graz|linz|salzburg|innsbruck|klagenfurt|st\.?\s*pölten|eisenstadt|bregenz|feldkirch|wels|villach|leoben'
advice = "Ask for an Austrian venue (usually your seat) or at least the defendant's seat."

[[rule]]
id = "foreign_law"
title = "Fremdes Recht / foreign governing law"
severity = "medium"
kind = "match"
pattern = '(anwendbar|anzuwenden|unterliegt|governed\s+by|subject\s+to\s+the\s+laws?\s+of)'
unless = 'österreich|austria|un-?kaufrecht|cisg'
advice = "Prefer Austrian law; check whether a foreign law clause is acceptable for this deal."

[[rule]]
id = "missing_dpa"
title = "Personenbezogene Daten ohne AVV / personal data without DPA"
severity = "high"
kind = "absent"
when = 'personenbezogen\w*\s+daten|dsgvo|personal\s+data|gdpr'
pattern = 'auftragsverarbeit|\bavv\b|data\s+processing\s+(agreement|addendum)|\bdpa\b|art\.?\s*28'
advice = "Add a data processing agreement (Art. 28 GDPR) as an annex when the other side processes personal data for you."

[[rule]]
id = "penalty"
title = "Vertragsstrafe / penalty"
severity = "medium"
kind = "match"
pattern = 'vertragsstrafe|konventionalstrafe|pönale|penalty|liquidated\s+damages'
advice = "Check amount and trigger; Austrian law allows judicial moderation (section 1336 ABGB), but negotiate a cap anyway."

[[rule]]
id = "unilateral_change"
title = "Einseitige Änderung / unilateral changes"
severity = "medium"
kind = "match"
pattern = 'einseitig\w*\s+(\w+\s+){0,4}(änder|anpass)|(may|reserves?\s+the\s+right\s+to)\s+(\w+\s+){0,4}(change|modify|amend)\s+(\w+\s+){0,3}(prices?|fees?|terms|conditions)\s+at\s+any\s+time'
advice = "Tie price changes to an index (e.g. VPI) and give a termination right for other changes."

[[rule]]
id = "exclusivity"
title = "Exklusivität / exclusivity"
severity = "low"
kind = "match"
pattern = 'exklusiv|ausschließlich\w*\s+(lieferant|anbieter|partner|vertrieb)|exclusivity|exclusive\s+(supplier|distribut\w*|partner|rights?|licen[cs]e|provider)|non-?compete|wettbewerbsverbot|konkurrenzklausel'
advice = "Limit scope, territory and duration."
"#;

#[derive(Debug, Clone, Deserialize)]
pub struct RuleDef {
    pub id: String,
    pub title: String,
    pub severity: String,
    pub kind: String,
    pub pattern: String,
    #[serde(default)]
    pub unless: Option<String>,
    #[serde(default)]
    pub when: Option<String>,
    #[serde(default)]
    pub advice: String,
}

#[derive(Debug, Deserialize)]
struct RuleFile {
    rule: Vec<RuleDef>,
}

pub struct Rule {
    pub def: RuleDef,
    pattern: Regex,
    unless: Option<Regex>,
    when: Option<Regex>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Finding {
    pub rule: String,
    pub title: String,
    pub severity: String,
    pub paragraph: usize,
    pub excerpt: String,
    pub advice: String,
}

fn rx(src: &str, id: &str) -> Result<Regex, String> {
    RegexBuilder::new(src)
        .case_insensitive(true)
        .size_limit(1 << 20)
        .build()
        .map_err(|e| format!("rule {id}: {e}"))
}

pub fn load(toml_text: &str) -> Result<Vec<Rule>, String> {
    let file: RuleFile = toml::from_str(toml_text).map_err(|e| format!("rule file: {e}"))?;
    let mut out = Vec::new();
    for def in file.rule {
        if !matches!(def.severity.as_str(), "high" | "medium" | "low") {
            return Err(format!(
                "rule {}: severity must be high, medium or low",
                def.id
            ));
        }
        let (unless, when) = match def.kind.as_str() {
            "match" => (
                def.unless.as_deref().map(|u| rx(u, &def.id)).transpose()?,
                None,
            ),
            "absent" => (
                None,
                Some(rx(
                    def.when
                        .as_deref()
                        .ok_or_else(|| format!("rule {}: absent rules need `when`", def.id))?,
                    &def.id,
                )?),
            ),
            other => {
                return Err(format!(
                    "rule {}: kind {other:?} (use match or absent)",
                    def.id
                ))
            }
        };
        out.push(Rule {
            pattern: rx(&def.pattern, &def.id)?,
            unless,
            when,
            def,
        });
    }
    if out.is_empty() {
        return Err("rule file has no [[rule]] entries".into());
    }
    Ok(out)
}

pub fn default_rules() -> Result<Vec<Rule>, String> {
    load(DEFAULT_RULES)
}

fn paragraphs(text: &str) -> Vec<&str> {
    text.split("\n\n")
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .collect()
}

fn excerpt(p: &str, start: usize, end: usize) -> String {
    let chars: Vec<(usize, char)> = p.char_indices().collect();
    let si = chars.iter().position(|(i, _)| *i >= start).unwrap_or(0);
    let ei = chars
        .iter()
        .position(|(i, _)| *i >= end)
        .unwrap_or(chars.len());
    let a = si.saturating_sub(60);
    let b = (ei + 60).min(chars.len());
    let mut s: String = chars[a..b]
        .iter()
        .map(|(_, c)| *c)
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if a > 0 {
        s.insert_str(0, "...");
    }
    if b < chars.len() {
        s.push_str("...");
    }
    s
}

pub fn scan(rules: &[Rule], text: &str) -> Vec<Finding> {
    let paras = paragraphs(text);
    let mut out = Vec::new();
    for r in rules {
        let finding = |i: usize, p: &str, s: usize, e: usize| Finding {
            rule: r.def.id.clone(),
            title: r.def.title.clone(),
            severity: r.def.severity.clone(),
            paragraph: i + 1,
            excerpt: excerpt(p, s, e),
            advice: r.def.advice.clone(),
        };
        match &r.when {
            None => {
                for (i, p) in paras.iter().enumerate() {
                    if let Some(m) = r.pattern.find(p) {
                        if r.unless.as_ref().is_some_and(|u| u.is_match(p)) {
                            continue;
                        }
                        out.push(finding(i, p, m.start(), m.end()));
                    }
                }
            }
            Some(when) => {
                if !r.pattern.is_match(text) {
                    if let Some((i, p, m)) = paras
                        .iter()
                        .enumerate()
                        .find_map(|(i, p)| when.find(p).map(|m| (i, *p, m)))
                    {
                        out.push(finding(i, p, m.start(), m.end()));
                    }
                }
            }
        }
    }
    let rank = |s: &str| match s {
        "high" => 0,
        "medium" => 1,
        _ => 2,
    };
    out.sort_by(|a, b| {
        (rank(&a.severity), a.paragraph, &a.rule).cmp(&(rank(&b.severity), b.paragraph, &b.rule))
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(text: &str) -> Vec<String> {
        let mut v: Vec<String> = scan(&default_rules().unwrap(), text)
            .into_iter()
            .map(|f| f.rule)
            .collect();
        v.sort();
        v
    }

    #[test]
    fn default_rules_compile() {
        assert!(default_rules().unwrap().len() >= 8);
    }

    #[test]
    fn german_clauses() {
        let t = "§ 7 Haftung\nDer Auftragnehmer haftet unbeschränkt für alle Schäden.\n\n§ 8 Der Auftragnehmer hält den Auftraggeber schad- und klaglos.\n\n§ 9 Der Vertrag verlängert sich automatisch um ein Jahr.\n\n§ 10 Gerichtsstand ist München. Es gilt deutsches Recht, anwendbar ist das BGB.";
        assert_eq!(
            ids(t),
            vec![
                "auto_renewal",
                "foreign_jurisdiction",
                "foreign_law",
                "indemnity",
                "unlimited_liability"
            ]
        );
    }

    #[test]
    fn austrian_venue_and_law_are_fine() {
        assert!(
            ids("Gerichtsstand ist Wien. Der Vertrag unterliegt österreichischem Recht.")
                .is_empty()
        );
        assert!(ids("Exclusive jurisdiction: courts of Vienna, Austria. This agreement is governed by Austrian law.").is_empty());
    }

    #[test]
    fn english_clauses_and_absent_dpa() {
        let t = "Supplier shall indemnify and hold harmless the Customer.\n\nThe Supplier processes personal data of the Customer's staff.\n\nThis agreement renews automatically. Venue: New York.";
        assert_eq!(
            ids(t),
            vec![
                "auto_renewal",
                "foreign_jurisdiction",
                "indemnity",
                "missing_dpa"
            ]
        );
        let with_dpa =
            format!("{t}\n\nAnnex 2: Data Processing Agreement pursuant to Art. 28 GDPR.");
        assert!(!ids(&with_dpa).contains(&"missing_dpa".to_string()));
        let f = scan(&default_rules().unwrap(), t);
        assert_eq!(f[0].severity, "high", "sorted by severity");
    }

    #[test]
    fn custom_rule_file_and_errors() {
        let rules =
            load("[[rule]]\nid='x'\ntitle='X'\nseverity='low'\nkind='match'\npattern='skonto'\n")
                .unwrap();
        assert_eq!(
            scan(&rules, "3 % Skonto bei Zahlung binnen 14 Tagen").len(),
            1
        );
        assert!(load(
            "[[rule]]\nid='x'\ntitle='X'\nseverity='urgent'\nkind='match'\npattern='a'\n"
        )
        .is_err());
        assert!(
            load("[[rule]]\nid='x'\ntitle='X'\nseverity='low'\nkind='absent'\npattern='a'\n")
                .is_err()
        );
        assert!(
            load("[[rule]]\nid='x'\ntitle='X'\nseverity='low'\nkind='match'\npattern='('\n")
                .is_err()
        );
    }
}
