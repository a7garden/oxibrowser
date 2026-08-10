/**
 * §5.3c — setting `location.hash` must fire a `hashchange` event.
 */
type CdpResult = { id?: number; error?: { message?: string }; result?: any };
const CDP = Number(process.argv[2] ?? 19340);
const MOCK = Number(process.argv[3] ?? 18099);
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
    if (nav === TIMED_OUT || (nav as any).error) { step("Page.navigate", false, JSON.stringify(nav)); return finish(); }
    step("Page.navigate", true);
    await sleep(2000);
    const r = await race(send("Runtime.evaluate", { expression: "window.__hash", returnByValue: true }), 5000);
    if (r === TIMED_OUT || (r as any).error) { step("hashchange fired", false, JSON.stringify(r)); return finish(); }
    const v = (r as any).result?.result?.value ?? "";
    const ok = typeof v === "string" && v.startsWith("hashchange:") && v.includes("#two") && v.includes(ORIGIN);
    step("hashchange listener fired on location.hash = '#two'", ok, "value=" + JSON.stringify(v));
    return finish();
  } catch (e: any) { console.error("exception", e); return finish(); }
});

function finish() {
  console.log(`\n=== hashchange e2e: ${pass} PASS / ${fail} FAIL ===`);
  process.exit(fail === 0 ? 0 : 1);
}
