//! Output formats. `md` writes the contract text as is; `typ` converts it to a
//! Typst source; `pdf` runs the Typst CLI on that source. No PDF is produced
//! without a real renderer.
//!
//! Text conventions understood by the converter: `# ` and `## ` headings,
//! `- ` bullet lines, blank lines between paragraphs. Everything else is
//! escaped and appears literally.

use std::path::Path;
use std::process::Command;

/// `CONTRACTCRAFT_TYPST` if set, else `typst` on PATH.
pub fn typst_bin() -> String {
    std::env::var("CONTRACTCRAFT_TYPST")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "typst".into())
}

fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if matches!(
            c,
            '\\' | '#'
                | '$'
                | '*'
                | '_'
                | '`'
                | '<'
                | '>'
                | '@'
                | '['
                | ']'
                | '~'
                | '='
                | '/'
                | '"'
        ) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

pub fn to_typst(title: &str, number: &str, body: &str) -> String {
    let mut out = String::from(
        "#set page(paper: \"a4\", margin: (x: 2.2cm, y: 2.4cm), numbering: \"1 / 1\")\n\
         #set text(font: (\"Segoe UI\", \"Arial\", \"Liberation Sans\"), size: 10.5pt, lang: \"de\")\n\
         #set par(justify: true)\n",
    );
    out.push_str(&format!(
        "#align(right)[#text(size: 8pt)[{}]]\n\n",
        esc(number)
    ));
    out.push_str(&format!(
        "#text(size: 16pt, weight: \"bold\")[{}]\n\n",
        esc(title)
    ));
    for line in body.lines() {
        let t = line.trim_end();
        if let Some(h) = t.strip_prefix("## ") {
            out.push_str(&format!("== {}\n", esc(h.trim())));
        } else if let Some(h) = t.strip_prefix("# ") {
            out.push_str(&format!("= {}\n", esc(h.trim())));
        } else if let Some(item) = t.trim_start().strip_prefix("- ") {
            out.push_str(&format!("- {}\n", esc(item.trim())));
        } else {
            let e = esc(t);
            // A leading + or "3. " would start a Typst list; an empty content block breaks it.
            let e = if e.starts_with('+') {
                format!("#[]{e}")
            } else if let Some((n, rest)) = e
                .split_once(". ")
                .filter(|(n, _)| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
            {
                format!("{n}#[]. {rest}")
            } else {
                e
            };
            out.push_str(&e);
            out.push('\n');
        }
    }
    out
}

/// Run `typst compile src out`. A missing binary gives an install hint, never a fake file.
pub fn compile_pdf(src: &Path, out: &Path) -> Result<(), String> {
    let bin = typst_bin();
    let res = Command::new(&bin).arg("compile").arg(src).arg(out).output();
    match res {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(format!(
            "PDF needs the Typst CLI ({bin} not found). Install it (winget install --id Typst.Typst) or set CONTRACTCRAFT_TYPST; md and typ output work without it."
        )),
        Err(e) => Err(format!("{bin}: {e}")),
        Ok(o) if !o.status.success() => Err(format!("typst failed: {}", String::from_utf8_lossy(&o.stderr).trim())),
        Ok(_) if !out.exists() => Err("typst reported success but wrote no PDF".into()),
        Ok(_) => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typst_source_escapes_and_structures() {
        let s = to_typst("Wartung #1", "C-2026-001", "# § 1 Gegenstand\nPreis: 1.200,00 EUR // netto\n\n## Details\n- Support *24/7*\n+ kein Listenpunkt\n3. Absatz");
        assert!(s.contains("= § 1 Gegenstand\n"));
        assert!(s.contains("== Details\n"));
        assert!(s.contains("- Support \\*24\\/7\\*\n"));
        assert!(
            s.contains("Preis: 1.200,00 EUR \\/\\/ netto"),
            "no Typst comment"
        );
        assert!(s.contains("#[]+ kein Listenpunkt"));
        assert!(s.contains("3#[]. Absatz"));
        assert!(s.contains("Wartung \\#1"));
    }

    #[test]
    fn missing_typst_is_a_clear_error() {
        std::env::set_var("CONTRACTCRAFT_TYPST", "D:/definitely/not/typst-xyz.exe");
        let dir = std::env::temp_dir();
        let err = compile_pdf(&dir.join("x.typ"), &dir.join("x-never.pdf")).unwrap_err();
        std::env::remove_var("CONTRACTCRAFT_TYPST");
        assert!(err.contains("Typst CLI"), "{err}");
        assert!(!dir.join("x-never.pdf").exists());
    }
}
