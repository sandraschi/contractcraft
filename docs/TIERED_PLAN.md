# ContractCraft - Tiered Execution Plan

**Parent Standard:** [`RUST_ARTCRAFT_STANDARD.md`](../../mcp-central-docs/standards/RUST_ARTCRAFT_STANDARD.md)  
**Menu Bar Standard:** [`CRAFT_MENU_BAR_STANDARD.md`](../../mcp-central-docs/standards/rust-artcraft/CRAFT_MENU_BAR_STANDARD.md)  
**Architecture Model:** 4-Tier ArtCraft Incremental Delivery  
**Status:** Active  

---

## Architecture Flow

```
Tier 1: Core Engine ──> Tier 2: MCP / CLI ──> Tier 3: Desktop UI ──> Tier 4: Menu Bar, Help & Packaging
(contractcraft-core)   (contractcraft-mcp)   (contractcraft-app)   (Hardened Workstation)
```

---

## Tier Specifications

### Tier 1 - Core Contract & Review Engine (`contractcraft-core`)
- [x] L1 `Command::dispatch` universal contract execution bus.
- [x] Versioned template engine with strict variable interpolation.
- [x] Enforced status machine (draft, review, signature, executed).
- [x] Deterministic clause risk scanner with DE/EN heuristic rules.
- [x] Cryptographic SHA-256 + Ed25519 document sealing and tamper verification.
- [x] Notice deadline calculator emitting cross-app bus events (`contract.deadline` -> TaskCraft).
- [x] `#![forbid(unsafe_code)]` and zero-panic clippy lints.

### Tier 2 - Headless Driver & FastMCP (`contractcraft-mcp`)
- [x] Unified CLI `contractcraft-cli` with `--json` output.
- [x] FastMCP stdio server communicating over JSON-RPC 2.0 (17 tools).
- [x] Typst / PDF renderer integration with fallback diagnostics.
- [x] Headless `--smoke` verification without window initialization.

### Tier 3 - Desktop Workstation UI (`contractcraft-app`)
- [x] Immediate-mode `egui` 0.37 / `wgpu` workstation.
- [x] Contract pipeline Kanban board by status.
- [x] Side-by-side redline diff viewer with clause risk highlights.
- [x] Notice deadline calendar and seal verification HUD.

### Tier 4 - Craft Menu Bar Standard, Help Suite & Hardened Verification
- [ ] Universal 8-menu bar (`File`, `Edit`, `View`, `Insert`, `Modify`, `Commands`, `Window`, `Help`) with `switch_on_hover`.
- [ ] Mandatory 5-dialog Help suite (`Command Palette...`, `Keyboard Shortcuts...`, `Project Files & Docs...`, `Licenses & Notices...`, `About ContractCraft`).
- [ ] Fleet UI/UX Pattern 2 adherence: viewport-centric navigation, top button row mapped to View toggles, panel visibility managed via `View` / `Window`.
- [ ] Tofu Guard verification (100% font glyph coverage, zero tofu placeholder boxes).
- [ ] Headless `--smoke` parity validation.
- [ ] Standalone release binary packaging and NSIS installer.
