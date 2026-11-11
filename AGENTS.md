# AGENTS.md - contractcraft

Contract templates, workflow, redlines, risk scan, seals and deadlines. Read first:
`mcp-central-docs/standards/RUST_ARTCRAFT_STANDARD.md`,
`mcp-central-docs/projects/craft-fleet-expansion/BACKOFFICE_CRAFT_SUITE.md` (section 4.5).

## Layout

- `crates/contractcraft-store` (L1): templates (versioned), contracts, immutable versions and seals, transitions, emitted-event log on SQLite WAL. Status machine and template fill live here.
- `crates/contractcraft-core` (L1): `Command` bus, `dispatch`, MCP catalog, `risk.rs` (TOML rules, default DE/EN set), `seal.rs` (SHA-256 + Ed25519), `render.rs` (md -> Typst, Typst CLI wrapper), redline (`similar`), deadlines, demo seed.
- `crates/contractcraft-mcp` (L2): `contractcraft` binary. CLI verbs map onto the same tool calls as MCP.
- `crates/contractcraft-app` (L3): egui window on `craft_biz_ui::app::Shell`. Changes only through `Frame::run/run_confirm/form` (dispatch); reads from the store, `risk::scan`, `next_deadline`. `COVERAGE` lists where each tool lives; a test fails when a tool has no place. Board drop rules mirror `Status::can_move_to` plus the seal requirement.
- Shared kernel: `../craft-biz` (deadline maths, money, bus, MCP loop, confinement).

## Invariants (never weaken)

- Status changes only along `Status::can_move_to`; terminate/cancel need a reason.
- Text changes only in draft, only as a new version. Versions and seals immutable (triggers).
- Executed requires a seal of the current version.
- The seal is tamper evidence. Never call it a signature with legal effect in code, docs or tool descriptions.
- Never produce a PDF without a real renderer (no placeholder bytes).
- Risk scan output always carries the "not legal advice" disclaimer.
- Events (`contract.deadline` -> taskcraft, `contract.executed` -> gaincraft) are sent once, recorded in `emitted`.
- The seal key never appears in output or logs.

## Docs (keep current in the same commit)

- `README.md`: what it does, quick start, MCP config, build.
- `HELP.md`: task-oriented how-to; update when a verb or workflow changes.
- `docs/PRD.md`: purpose, boundaries, honest gaps, next steps; update "Today" and "Gaps" with every release.
- `docs/TOOLS.md`: generated. After any tool change run `just docs`; the `tools_doc_is_current` test fails otherwise.
- `CHANGELOG.md`: one entry per change set.
- Window: README "Desktop window", HELP "In the window", `COVERAGE` in the app crate.
- CLI help: `about`, argument doc comments and `AFTER_HELP` in the mcp crate's `main.rs`.

## Rules

- Zero-panic: unwrap/expect/panic/todo/unimplemented denied outside tests.
- MCP mode: stdout is JSON-RPC only; diagnostics to stderr.
- File tools (body_path, rules_path, render, verify file) go through `Confinement::check`.
- Demo data only via `demo-seed`, only on an empty database, labelled DEMO. Mock names only.
- Every bug fix ships with a regression test.

## Repo hygiene

- LF line endings only (`.gitattributes`). CRLF breaks commit hooks.
- Single trunk `main`. No agent branches or leftover WIP branches.
- No em dash characters in code, docs or commit messages.
- No `*.bak`, `*.db`, `*.key`, rendered outputs, `target/`, binaries in git.

## Gates

1. `cargo clippy --workspace --all-targets --all-features -- -D warnings`
2. `cargo test --workspace`
3. `cargo fmt --all --check`
4. MCP probe: `initialize` -> `tools/list` -> `tools/call`, stdout pure JSON
5. `Cargo.lock` committed, clean `git status`
6. `CHANGELOG.md` entry; README / HELP / PRD updated if behaviour changed
