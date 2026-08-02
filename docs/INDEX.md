# OxiBrowser Docs Index

> Canonical unified design system: `project-oxi/.github/DESIGN.md`.
> Each project's own `DESIGN.md` is now a project-specific working fork with a pointer header.

## Layout

| Path | Purpose | Files |
|------|---------|------:|
| `./` | **Top-level architecture, design rationale, quickstart, roadmap.** | 7 |
| `design/` | Focused design notes for in-flight subsystems (observability, search-as-library). | 2 |
| `designs/` | Dated design documents per planning cycle (v0.3 → v0.6, plus topic designs). | 13 |
| `archive/` | Superseded versions, scoped topic designs, and transient agent reports. | 13 |

Total active docs (root + `design/` + `designs/`): **22** `.md` files.
Archived (out of root, kept for history): 9 in `archive/`, 4 in `archive/transient/`.

## Top-level docs (root)

- **`DESIGN.md`** — OxiBrowser design rationale (why pure-Rust headless, Servo ecosystem, CDP-compat, agent workloads). *Project-specific fork; canonical unified design system lives at `project-oxi/.github/DESIGN.md`.*
- **`ARCHITECTURE.md`** — Module layout, lifecycle, threading model, integration points.
- **`CDP.md`** — Chrome DevTools Protocol surface supported by OxiBrowser, mappings, and quirks.
- **`QUICKSTART.md`** — First-run, embedding, and example agent usage.
- **`roadmap-v0.5.md`** — v0.5 milestone plan; later milestones have moved to the dated designs in `designs/`.
- **`search-command-proposal.md`** — Proposal for a search-driven CLI surface.
- **`design-agent-layout-eval.md`** — Layout evaluation notes produced during agent-mode design passes.

## `design/` — focused subsystem notes

- `observability.md` — Tracing, metrics, and logging strategy.
- `search-as-library.md` — Embedding OxiBrowser's search/index primitives.

## `designs/` — dated design documents (chronological)

| Date | Topic |
|------|-------|
| `2026-05-16-cli-enhancement-design.md` | CLI ergonomics pass (v0.3 era) |
| `2026-06-04-oxibrowser-observability.md` | Observability design v1 |
| `2026-06-04-oxibrowser-observability-followup.md` | Observability follow-ups |
| `2026-06-25-pure-rust-stealth.md` | Pure-Rust stealth / detection-resistance design |
| `2026-06-real-web-readiness.md` | Real-web readiness gap analysis |
| `v0.3-headless-browser.md` | v0.3 architecture baseline |
| `v0.4-production-grade.md` | v0.4 hardening targets |
| `v0.5-completion-master.md` | v0.5 completion master plan |
| `v0.6-cdp-events-perf.md` | v0.6 CDP event-throughput work |
| `agent-os-sdk.md` | Agent-OS SDK surface |
| `merge-guide.md` | Merge / contribution walkthrough |
| `session-a-web-platform.md` | Session A working notes — web platform side |
| `session-b-cdp-perf.md` | Session B working notes — CDP perf side |

## `archive/` — superseded designs and transient reports

- **Superseded designs** (kept for history): `CLI-V2-DESIGN.md`, `CLI-V2-REMAINING.md`, `DOM_API_DESIGN.md`, `FIX_DESIGN.md`, `HEADLESS_ROADMAP.md`, `IMPROVEMENT_DESIGN.md`, `SCENARIO_TEST_REPORT.md`, `V0.7.0_DESIGN.md`, `phase3-spec.md`.
- **`archive/transient/`** — Agent-generated transient reports (`progress.md`, `.oxi-explore-parser-report.md`, `.oxi-fixraf-final.md`, `.oxi-fixraf-result.md`). Moved out of repo root; not authoritative.

## Canonical design pointer

For the unified oxi design system (tokens, typography, components, motion, dark mode, accessibility), see:

> **`project-oxi/.github/DESIGN.md`** (v1.0, dated 2026-07-31)

OxiBrowser-specific design rationale remains at `docs/DESIGN.md`.
