# contractcraft

![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)
![Rust 1.90+](https://img.shields.io/badge/rust-1.90%2B-orange?logo=rust)
![MCP stdio](https://img.shields.io/badge/MCP-stdio-purple)
![ArtCraft harness](https://img.shields.io/badge/ArtCraft-harness-green)

Contract templates, review workflow, redlines, a clause risk scan, tamper-evident seals and notice deadlines for a small Austrian business. One SQLite file, one binary that is both CLI and MCP server. Part of the back-office craft suite (replaces Word templates in a folder, ad-hoc review mails and the "when can we cancel this?" spreadsheet).

Status: **v0.2.0** (store + CLI + MCP + desktop window, tested). Synthetic demo data only. **Not legal advice**: the risk scan points a human reviewer at clauses, nothing more.

- How to use it day to day: [HELP.md](HELP.md)
- What it is for and where it stops: [docs/PRD.md](docs/PRD.md)
- Every MCP tool: [docs/TOOLS.md](docs/TOOLS.md) (generated, checked by a test)
- Changes: [CHANGELOG.md](CHANGELOG.md)

## What it does

- **Templates** with declared variables (`{{kunde}}`, required/default). Placeholders and declarations must match; filling refuses typos, missing required values and leftover placeholders. Template edits create versions; contracts remember which version they came from.
- **Status machine:** draft -> under_review -> pending_signature -> executed -> terminated / expired (cancelled from any open state). Everything else is refused; every move is logged with actor and note.
- **Versions:** text changes only in draft and only as a new version. Versions and seals are immutable (SQLite triggers).
- **Redline:** line diff between versions plus the risk delta ("this edit adds unlimited liability").
- **Risk scan:** deterministic DE + EN rules in TOML: unlimited liability, Schad- und Klagloshaltung / indemnity, automatic renewal, venue or governing law outside Austria, personal data without AVV / DPA, penalties, unilateral changes, exclusivity. Bring your own rule file with `--rules`.
- **Seal:** SHA-256 of the exact text plus an Ed25519 signature over (number, version, hash, time). Executing a contract requires a seal of the current version. `verify` checks the stored text and, with `--file`, that an exported copy is byte-identical. This is tamper evidence, **not** a legally binding e-signature (no QES / ID Austria).
- **Render:** `md` (byte-identical to the sealed text), `typ` (Typst source), `pdf` via the Typst CLI. Without Typst the PDF verb fails with an install hint; no fake PDF.
- **Deadlines:** last notice day of auto-renewing contracts (month-end rule from craft-biz) or the end of a fixed term. `--emit` sends each deadline once to taskcraft (`contract.deadline` becomes an issue with a due date). Executing sends `contract.executed` to gaincraft.

## Quick start

```powershell
contractcraft demo-seed
contractcraft list
contractcraft show C-2026-001
contractcraft scan C-2026-002
contractcraft redline C-2026-003
contractcraft deadlines --days 60 --emit     # taskcraft inbox-drain picks them up
contractcraft draft "Wartung Kanzlei" "Mock & Partner OG" --template "Wartungsvertrag (DEMO)" --set kunde="Mock & Partner OG" --set leistung="Wartung" --set entgelt="300,00 EUR" --set beginn=01.01.2027
contractcraft terms C-2026-004 --start 01.01.2027 --months 12 --renew 12 --notice 3 --value "3.600,00"
contractcraft move C-2026-004 under_review
contractcraft seal C-2026-004
contractcraft render C-2026-001 D:\Temp\c1.md
contractcraft verify C-2026-001 --file D:\Temp\c1.md
```

Template variables on the CLI: `--var kunde!` (required), `--var ort=Wien` (default), `--var notiz` (optional).

Database: `--db PATH`, else `$CONTRACTCRAFT_DB`, else `%LOCALAPPDATA%\contractcraft\contractcraft.db`. Seal key: `--key`, else `seal.key` next to the database (created on first seal; keep it out of shared backups). Typst: `typst` on PATH or `CONTRACTCRAFT_TYPST`.

## Desktop window

`contractcraft-app` is the window, laid out like a contract lifecycle tool. Every action goes through the same dispatch as CLI and MCP; all 17 tools have a place in it (checked by a test).

- **Pipeline:** kanban by status (draft, under review, pending signature, executed, ended) with KPI tiles for active annual value, open work, high risks and deadlines inside 60 days. Dragging a card changes the status; only allowed steps are accepted, and executing is refused with "seal version N first" until the current version is sealed. Executing, terminating and cancelling ask for confirmation.
- **All contracts:** sortable, filterable table with status, annual value, risk count and next deadline.
- **Contract page:** status stepper with the allowed next steps; tabs for the text (numbered paragraphs, risky paragraphs highlighted with rule and advice, editing in draft saves a new version), versions with an inline redline, risks, seals (seal, verify, verify an exported file), deadlines and the transition history. Side panel: counterparty, terms (edit before execution), export to md / Typst / PDF.
- **Deadlines:** next notice date or expiry per contract; "Send to taskcraft" emits `contract.deadline` events.
- **Templates:** variables with required markers, "Use template" opens a prefilled draft form.
- **Risk check:** paste a contract you received and scan it; nothing is stored.
- Ctrl+K command palette for any tool, signals dock (outgoing events, retry), Ctrl+mouse wheel zoom, high-contrast light and dark themes, DE/EN.

```powershell
contractcraft-app                      # default database
contractcraft-app --demo --view 0     # demo data, pipeline
contractcraft-app --select C-2026-003 --view 1
```

## MCP

```json
{ "mcpServers": { "contractcraft": { "command": "D:/Dev/cargo-target/release/contractcraft.exe", "args": ["mcp", "--allow-path", "D:/Dev/contracts"] } } }
```

17 tools, all `contract_*`; see [docs/TOOLS.md](docs/TOOLS.md). Agent actions are logged with actor `agent`.

## Build

```powershell
just ci
just build-release
```

Needs `../craft-biz` checked out next to this repo.

## Not done (honest list)

- The window has no classic menu bar (File / Edit / View ...); actions live in the top bar, side panel and the Ctrl+K palette.
- PDF only through the Typst CLI (not installed on Goliath yet). The md -> Typst converter knows headings, bullets and paragraphs; no tables or bold.
- No LLM review (planned via craft-ai-core, always labelled advisory).
- Seal key is a plain file, not Windows Credential Manager yet.
- No QES / ID Austria / A-Trust signatures; parties still sign outside the app.
- No word-level redline (line level only); no DOCX import.
