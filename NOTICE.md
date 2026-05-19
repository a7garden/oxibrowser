# Attribution

OxiBrowser is a Rust port of [Lightpanda](https://github.com/lightpanda-io/browser),
a headless browser built in Zig, licensed under the GNU Affero General Public License v3.

The original Lightpanda project is Copyright (c) 2024 lightpanda contributors.

The following architectural concepts and module structures are directly derived from Lightpanda:

- **Browser → Session → Page → Frame hierarchy** — the core browsing context model
- **CDP protocol implementation** — domain dispatch, event handling, and message structure
- **Per-domain handler modules** — mirrors Lightpanda's `src/cdp/domains/` layout
- **DOM types and parsing pipeline** — mirrors Lightpanda's `src/dom/` architecture
- **Session lifecycle and navigation** — history, storage, and page management

All original Lightpanda code is Copyright (c) 2024 lightpanda contributors,
used under the terms of the GNU Affero General Public License v3.

OxiBrowser re-implements these concepts in Rust with:
- **Language**: Rust instead of Zig
- **JS Engine**: Servo/SpiderMonkey instead of V8
- **HTML Parser**: html5ever instead of custom Zig parser
- **HTTP Client**: reqwest instead of custom implementation
- **Async Runtime**: Tokio instead of Zig's event loop

As a derivative work of an AGPL-3.0 project, OxiBrowser is also licensed under
AGPL-3.0-only. See LICENSE for the full license text.
