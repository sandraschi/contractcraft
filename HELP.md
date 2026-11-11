# contractcraft help

Task-oriented guide. Every command also exists as an MCP tool ([docs/TOOLS.md](docs/TOOLS.md)); add `--json` for machine-readable output; `contractcraft <command> --help` lists all options.

The risk scan points a human reviewer at clauses. It is **not legal advice**. The seal is tamper evidence, **not** a legally binding electronic signature.

## In the window

Start `contractcraft-app` (`--demo` loads demo data into an empty database).

- **Move a contract:** drag its card on the Pipeline. A refused drop shows why ("Seal version 2 first", "Draft -> Executed is not an allowed step"). Or open the contract and use the arrow buttons next to the status stepper; disabled buttons explain themselves on hover.
- **Open a contract:** double-click a card, click a row in All contracts or Deadlines.
- **Edit text:** contract page > Text > Edit text (draft only) > Save as new version, with a note.
- **Compare versions:** Versions tab, pick two versions; removed lines are red and struck through, added lines green. "Redline report" adds the risk delta.
- **Seal and verify:** side panel > Seal vN, Verify seals, Verify a file (checks an exported copy).
- **Terms:** side panel > Edit terms (before execution). The next deadline appears in the side panel and the Deadlines tab.
- **Deadlines to taskcraft:** Deadlines > Send to taskcraft (90 days). Each deadline is sent once.
- **New contract from a template:** Templates > Use template; the form is prefilled with the template's variables.
- **Check a contract you received:** Risk check, paste the text, Check.
- **Anything else:** Ctrl+K opens the command palette with every tool. Ctrl+mouse wheel zooms, Ctrl+0 resets.

## Templates

Write the text with `{{placeholders}}`, then register it with its variables: `name!` is required, `name=Wien` has a default, `name` is optional.

```powershell
contractcraft template add "Wartungsvertrag" D:\Vorlagen\wartung.md --var kunde! --var leistung! --var entgelt! --var beginn! --var ort=Wien
contractcraft template list
contractcraft template show "Wartungsvertrag"
```

Every placeholder must be declared and every declared variable used. Adding a template with an existing name creates a new version; contracts remember which version they came from.

Text conventions for rendering: `# ` and `## ` headings, `- ` bullets, blank lines between paragraphs.

## Drafting

```powershell
contractcraft draft "Wartung Server" "Mock GmbH" --template "Wartungsvertrag" --set kunde="Mock GmbH" --set leistung="Wartung" --set entgelt="800,00 EUR" --set beginn=01.01.2027
contractcraft draft "Kaufvertrag Laptop" "Anna Mockova" --file D:\Temp\kauf.md
contractcraft list
contractcraft show C-2026-004
```

Typos in `--set` names and missing required values are errors, not silent blanks.

## Review

```powershell
contractcraft scan C-2026-004
contractcraft scan --file D:\Temp\their-draft.txt                 # someone else's text
contractcraft scan C-2026-004 --rules D:\Vorlagen\rules.toml       # your own rule file
contractcraft edit C-2026-004 D:\Temp\wartung-v2.md --note "Haftung angepasst"
contractcraft redline C-2026-004                                   # previous vs current, plus risk delta
```

The text can only change in `draft`, and every change is a new version.

## Workflow

```powershell
contractcraft move C-2026-004 under_review
contractcraft move C-2026-004 draft --note "Kunde will Änderungen"
contractcraft move C-2026-004 pending_signature
contractcraft seal C-2026-004
contractcraft move C-2026-004 executed
contractcraft move C-2026-004 terminated --note "Kündigung zum 31.12.2027"
```

`executed` needs a seal of the current version. Terminating or cancelling needs a reason. Executing tells gaincraft (account value and deadlines).

## Terms and deadlines

```powershell
contractcraft terms C-2026-004 --start 01.01.2027 --months 12 --renew 12 --notice 3 --value "9.600,00"
contractcraft deadlines --days 90
contractcraft deadlines --days 90 --emit        # each deadline once to taskcraft
```

Auto-renewing contracts get their last notice day (month-end rule: a 31.12. term end with 3 months notice gives 30.09.). Fixed terms (`--renew 0`) get their end date. Terms are fixed once the contract is executed.

## Seals and copies

```powershell
contractcraft render C-2026-004 D:\Temp\c4.md          # byte-identical to the sealed text
contractcraft render C-2026-004 D:\Temp\c4.typ --format typ
contractcraft render C-2026-004 D:\Temp\c4.pdf --format pdf    # needs the Typst CLI
contractcraft verify C-2026-004
contractcraft verify C-2026-004 --file D:\Temp\c4.md   # is this copy exactly what was sealed?
```

The seal key is `seal.key` next to the database (or `--key`). Keep it out of shared backups.

## Troubleshooting

- **"cannot go from draft to executed (allowed: ...)"**: follow the workflow; the message lists the allowed next steps.
- **"seal version N before executing it"**: run `contractcraft seal`.
- **"PDF needs the Typst CLI"**: `winget install --id Typst.Typst`, or render `md` / `typ`.
- **MCP cannot read or write files**: start the server with `--allow-path <folder>`.
