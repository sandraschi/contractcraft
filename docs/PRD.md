# contractcraft PRD

## What it is

Contract templates, a review workflow, redlines, a deterministic clause risk scan, tamper-evident seals and notice deadlines for a small Austrian business. Pure Rust, ArtCraft pattern. Suite plan: `BACKOFFICE_CRAFT_SUITE.md`, section 4.5.

## Users

- The owner who writes, reviews and keeps contracts and must not miss a notice deadline.
- Agents over MCP: draft from templates, scan risk, compare versions, watch deadlines. Agent actions are logged with actor `agent`.

## Jobs

1. Produce contracts from vetted templates without placeholder mistakes.
2. See risky clauses and what an edit changed.
3. Prove later that a copy is exactly what was agreed.
4. Never miss a Kündigungsfrist.

## Today (v0.1.0)

- Versioned templates with strict variables; drafts from templates or own text.
- Enforced status machine; text changes only in draft as new versions; versions and seals immutable by trigger; executing needs a seal.
- Risk scan: 9 default DE/EN rules, user TOML rules, disclaimer on every result; redline with risk delta.
- SHA-256 + Ed25519 seal and verify (also of exported files); md, Typst source and (with the Typst CLI) PDF output.
- Deadlines with the craft-biz month-end rule; `contract.deadline` -> taskcraft (verified end to end); `contract.executed` -> gaincraft.
- 17 MCP tools ([TOOLS.md](TOOLS.md), generated and test-checked).

## Boundaries

- No legal advice; the scan only points at clauses.
- No QES / ID Austria / A-Trust; the seal is tamper evidence, parties still sign.
- No fake output: PDF only through a real renderer.
- Wrap renderers (Typst, later WordCraft/PdfCraft), do not reimplement them.

## Gaps (honest)

- No egui window.
- Typst not installed on Goliath yet, so PDF is untested end to end.
- md -> Typst converter knows headings, bullets and paragraphs only.
- Seal key is a plain file (Credential Manager planned).
- No LLM review, no word-level redline, no DOCX import.
- Rules not reviewed by a lawyer; synthetic data only.

## Next

1. Install Typst; add a PDF test that runs when the CLI is present.
2. egui app: contract list, side-by-side redline with risk panel, deadline calendar.
3. Lawyer review of the rule set; rules for Gewährleistung waivers and payment terms.
