# Changelog

## 0.1.0 - 2026-11-09

- Tier 1: contractcraft-store (versioned templates with strict variables, contracts with an enforced status machine, immutable versions and seals, transition log), contractcraft-core (risk rules DE/EN in TOML, Ed25519 seal and verify, redline with risk delta, md/Typst/PDF render via the Typst CLI, deadlines with taskcraft and gaincraft events, demo seed), `contractcraft` binary (CLI with `--json`, MCP stdio, 17 tools).
- 22 tests; live chain verified: `deadlines --emit` -> taskcraft `inbox-drain` -> issue with due date 30.11.2026.
