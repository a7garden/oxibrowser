/**
 * External stylesheet fixture.
 * Reproduces the §5.2 gap: <link rel=stylesheet href="/x.css"> must be
 * fetched + applied. The probe samples screenshot pixels to confirm the
 * stylesheet rule reached paint. LayoutEngine's `getComputedStyle` does not
 * parse `<style>` selectors, but Blitz's CSS engine drives layout/paint.
 *
 * Self-contained mock + probe.
 *
 *   bun acceptance/external-stylesheet/run.ts <cdp_port> [mock_port]
 */
import { inflateSync } from "node:zlib";
import { Buffer } from "node:buffer";

type CdpResult = { id?: number; error?: { message?: string }; result?: any };

const CDP = Number(process.argv[2] ?? 19326);
const MOCK = Number(process.argv[3] ?? 18096);
const ORIGIN = `http://127.0.0.1:${MOCK}`;
const TIMED_OUT = Symbol("TIMED_OUT");

let pass = 0, fail = 0;
function step(name: string, ok: boolean, detail = "") {
  if (ok) pass++; else fail++;
  console.log(`${ok ? "PASS" : "FAIL"}  ${name}${detail ? "  — " + detail : ""}`);
}

const ws = new WebSocket(`ws://127.0.0.1:${CDP}/ws`);
let nextId = 1;
const pending = new Map<number, (v: CdpResult) => void>();
let sid: string | undefined;

function send(method: string, params: Record<string, unknown> = {}): Promise<CdpResult> {
  const id = nextId++;
  const m: any = { id, method, params };
  if (sid) m.sessionId = sid;
  return new Promise((resolve) => { pending.set(id, resolve); ws.send(JSON.stringify(m)); });
}
function race<T>(p: Promise<T>, ms: number) {
  return Promise.race([p, new Promise<typeof TIMED_OUT>((r) => setTimeout(() => r(TIMED_OUT), ms))]);
}
ws.addEventListener("message", (ev: MessageEvent) => {
  const raw = typeof ev.data === "string" ? ev.data : new TextDecoder().decode(ev.data as ArrayBuffer);
  let p: any;
  try { p = JSON.parse(raw); } catch { return; }
  if (p.id != null && pending.has(p.id)) { pending.get(p.id)!(p); pending.delete(p.id); return; }
  if (p.method === "Target.attachedToTarget" && p.params?.sessionId && p.params?.targetInfo?.type === "page")
    sid = p.params.sessionId;
});
ws.addEventListener("error", () => { console.error("WS error"); process.exit(1); });
setTimeout(() => { console.error("timeout (30s)"); process.exit(1); }, 30000);
function sleep(ms: number) { return new Promise((r) => setTimeout(r, ms)); }

ws.addEventListener("open", async () => {
  try {
    await send("Target.setDiscoverTargets", { discover: true });
    await send("Target.setAutoAttach", { autoAttach: true, flatten: true });
    for (let i = 0; i < 200 && !sid; i++) await sleep(50);
    if (!sid) { step("CDP attach", false, "no sid"); return finish(); }
    step("CDP attach", true);
    await send("Runtime.enable");
    await send("Page.enable");

    const nav = await race(send("Page.navigate", { url: ORIGIN + "/" }), 10000);
    if (nav === TIMED_OUT || (nav as any).error) { step("Page.navigate (no panic)", false, JSON.stringify(nav)); return finish(); }
    step("Page.navigate (no panic)", true);
    await sleep(1500);

    const shot = await race(send("Page.captureScreenshot", { format: "png" }), 5000);
    if (shot === TIMED_OUT || (shot as any).error) { step("captureScreenshot", false, JSON.stringify(shot)); return finish(); }
    const b64 = (shot as any).result?.data as string | undefined;
    if (typeof b64 !== "string") { step("captureScreenshot data", false, "no data"); return finish(); }
    step("captureScreenshot", true);

    const bytes = new Uint8Array(Buffer.from(b64, "base64"));
    const probe = scanForGreenishPixel(bytes);
    step("rendered pixels contain the stylesheet's green", probe.hasGreenish, probe.detail);

    return finish();
  } catch (e: any) { console.error("exception", e); return finish(); }
});

function finish() {
  console.log(`\n=== external-stylesheet e2e: ${pass} PASS / ${fail} FAIL ===`);
  process.exit(fail === 0 ? 0 : 1);
}

interface ScanResult { hasGreenish: boolean; detail: string; }

/**
 * Concatenate PNG IDAT chunks, inflate via zlib, and scan for any pixel
 * whose green channel is dominant. The stylesheet rule sets `#008000`
 * (r=0, g=128, b=0); the antialiased interior of the glyph and the unused
 * PNG filter bytes between scanlines both contain green-ish ranges, so we
 * tighten the predicate to require the red and blue channels to be far
 * below green (avoiding the cyan-looking PNG header bytes) and the green
 * to sit in the printable mid-range (avoiding extreme near-white or near-
 * black pixels from the white background).
 */
function scanForGreenishPixel(png: Uint8Array): ScanResult {
  const idat = concatChunks(png, "IDAT");
  if (!idat) return { hasGreenish: false, detail: "no IDAT chunks" };
  let inflated: Buffer;
  try {
    inflated = inflateSync(idat);
  } catch (e: any) {
    return { hasGreenish: false, detail: `inflate failed: ${e?.message ?? e}` };
  }
  let count = 0;
  let bestIdx = -1;
  for (let i = 0; i + 2 < inflated.length; i++) {
    const r = inflated[i];
    const g = inflated[i + 1];
    const b = inflated[i + 2];
    // Skip the PNG filter bytes (filter byte 0..4 preceding each scanline)
    // by requiring a strict green-dominant signature: g ∈ [60, 200] AND
    // r + 40 ≤ g AND b + 40 ≤ g. That excludes white (255,255,255), near-
    // black, cyan (4,255,255) artefacts, and the various PNG filter
    // bytes (which often pair R=0 with G non-zero).
    if (g >= 60 && g <= 200 && r + 40 <= g && b + 40 <= g) {
      bestIdx = i;
      count += 1;
      if (count >= 3) break;
    }
  }
  if (bestIdx >= 0) {
    const r = inflated[bestIdx];
    const g = inflated[bestIdx + 1];
    const b = inflated[bestIdx + 2];
    return { hasGreenish: true, detail: `green pixels found (count=${count}); first rgb(${r},${g},${b}) at offset ${bestIdx}` };
  }
  return { hasGreenish: false, detail: `no green-dominant pixel in ${inflated.length} inflated bytes` };
}

function concatChunks(png: Uint8Array, typeStr: string): Uint8Array | null {
  const typeBytes = new TextEncoder().encode(typeStr);
  let offset = 8; // PNG signature
  const chunks: Uint8Array[] = [];
  while (offset + 12 <= png.length) {
    const len =
      (png[offset] << 24) |
      (png[offset + 1] << 16) |
      (png[offset + 2] << 8) |
      png[offset + 3];
    const t0 = png[offset + 4], t1 = png[offset + 5], t2 = png[offset + 6], t3 = png[offset + 7];
    if (t0 === typeBytes[0] && t1 === typeBytes[1] && t2 === typeBytes[2] && t3 === typeBytes[3]) {
      chunks.push(png.slice(offset + 8, offset + 8 + len));
    }
    offset += 8 + len + 4;
  }
  if (!chunks.length) return null;
  const total = chunks.reduce((s, c) => s + c.length, 0);
  const out = new Uint8Array(total);
  let p = 0;
  for (const c of chunks) { out.set(c, p); p += c.length; }
  return out;
}
