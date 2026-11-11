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

## UI/UX Architecture & Menu Bar Standard

In accordance with [`CRAFT_MENU_BAR_STANDARD.md`](../../mcp-central-docs/standards/rust-artcraft/CRAFT_MENU_BAR_STANDARD.md):
- **Pattern 2 (Viewport-Centric Craft Workstation):** Persistent contract document editor / redline comparison canvas with clause risk inspector. Top quick button row acts as accelerators into the `View` menu. To add a new contract view or risk analysis panel:
  1. Register a visibility toggle in the `View` or `Window` menu.
  2. Add an entry in the sidebar Template / Contract toolbox.
  3. Wire the action directly into the universal `Command::dispatch` bus.
- **Top Menu Bar Hierarchy:**
  - `File`: New Contract (`Ctrl+N`), Open Contract (`Ctrl+O`), Open Recent ▸, Save Draft (`Ctrl+S`), Save As... (`Ctrl+Shift+S`), Export Typst / PDF (`Ctrl+E`), Close (`Ctrl+W`), Quit (`Ctrl+Q`).
  - `Edit`: Undo (`Ctrl+Z`), Redo (`Ctrl+Y`), Cut, Copy, Paste, Select All, Preferences (`Ctrl+,`).
  - `View`: Contract Pipeline Kanban, Document Text Editor, Side-by-Side Redline Diff, Risk Findings Panel, Deadline Calendar, Theme (`Dark`/`Light`).
  - `Insert`: Add Clause, Insert Variable Placeholder, Add Party Signature Block, Add Notice Deadline.
  - `Modify`: Bump Version, Update Terms, Accept Redline Change, Reject Modification.
  - `Commands`: Run Risk Scan, Compute Redline Diff, Apply Ed25519 Seal, Verify Seal, Render PDF.
  - `Window`: Reset Layout, Document Viewport, Clause Inspector, Deadline Dock.
  - `Help` (Mandatory 5-Dialog Suite): `Command Palette...` (`Ctrl+K`), `Keyboard Shortcuts...` (`F1`), `Project Files & Docs...`, `Licenses & Notices...`, `About ContractCraft`.
- **Behavioral Mechanics:** Smooth `switch_on_hover` between top-level menus, zero-unsafe immediate-mode rendering, and headless `--smoke` parity.
