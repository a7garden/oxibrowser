/**
 * W3b — nested iframe end-to-end.
 *
 * Navigates to main → checks Page.getFrameTree reports BOTH child1 and
 * child2 (recursive frame tree) → evaluates in each context via
 * `Runtime.evaluate { contextId: ... }` and reads the `<h? id="t">` text.
 *
 * Pre-fix expectation: getFrameTree reports only one descendant; child2
 * context is unreachable. Post-fix expectation: child2 visible and
 * reachable.
 *
 *   bun acceptance/nested/run.ts <cdp_port> [mock_port]
 */
type CdpResult = { id?: number; error?: { message?: string }; result?: any };

const CDP = Number(process.argv[2] ?? 19322);
const MOCK = Number(process.argv[3] ?? 18093);
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
  return new Promise((resolve) => {
    pending.set(id, resolve);
    ws.send(JSON.stringify(m));
  });
}
function race<T>(p: Promise<T>, ms: number) {
  return Promise.race([p, new Promise<typeof TIMED_OUT>((r) => setTimeout(() => r(TIMED_OUT), ms))]);
}
ws.addEventListener("message", (ev) => {
  const raw = typeof ev.data === "string" ? ev.data : new TextDecoder().decode(ev.data as ArrayBuffer);
  let parsed: any;
  try { parsed = JSON.parse(raw); } catch { return; }
  if (parsed.id != null && pending.has(parsed.id)) { pending.get(parsed.id)!(parsed); pending.delete(parsed.id); return; }
  if (parsed.method === "Target.attachedToTarget" && parsed.params?.sessionId && parsed.params?.targetInfo?.type === "page")
    sid = parsed.params.sessionId;
});
ws.addEventListener("error", () => { console.error("WS error"); process.exit(1); });
setTimeout(() => { console.error("timeout 30s"); process.exit(1); }, 30000);

async function evalIn(expr: string, contextId?: number): Promise<{ value?: any; error?: string }> {
  const p: any = { expression: expr, returnByValue: true };
  if (contextId !== undefined) p.contextId = contextId;
  const r = await send("Runtime.evaluate", p);
  if (r.error) return { error: r.error.message };
  const result = r.result?.result;
  if (result?.exceptionDetails) return { error: result.exceptionDetails.text ?? "exception" };
  return { value: result?.value };
}
function sleep(ms: number): Promise<void> { return new Promise(r => setTimeout(r, ms)); }
function collectFrames(node: any, acc: any[] = []) {
  acc.push(node.frame);
  for (const c of node.childFrames || []) collectFrames(c, acc);
  return acc;
}

ws.addEventListener("open", async () => {
  try {
    await send("Target.setDiscoverTargets", { discover: true });
    await send("Target.setAutoAttach", { autoAttach: true, flatten: true });
    for (let i = 0; i < 200 && !sid; i++) await sleep(50);
    if (!sid) { step("CDP attach", false, "no sid"); return finish(); }
    step("CDP attach", true);
    await send("Runtime.enable");
    await send("Page.enable");
    step("enable domains", true);

    const nav = await race(send("Page.navigate", { url: ORIGIN + "/" }), 10000);
    if (nav === TIMED_OUT || (nav as any).error) { step("Page.navigate", false, JSON.stringify(nav)); return finish(); }
    step("Page.navigate", true);
    await sleep(2500);

    const ftR = await race(send("Page.getFrameTree"), 5000);
    if (ftR === TIMED_OUT || (ftR as any).error) { step("Page.getFrameTree", false, JSON.stringify(ftR)); return finish(); }
    const ft = (ftR as any).result?.frameTree;
    const all = collectFrames(ft);
    step("getFrameTree returns >= 3 frames (root + child1 + child2)", all.length >= 3, `frames=${all.map(f => f.id).join(",")}`);

    // Each frame's title id=t; identifies the level.
    const mainR = await race(evalIn("document.getElementById('t').textContent"), 5000);
    step("Runtime.evaluate(main) reads 'main'", !!(mainR as any).error === false && (mainR as any).value === "main", JSON.stringify(mainR));

    // child1
    const c1R = await race(evalIn("document.getElementById('t').textContent", 2), 5000);
    step("Runtime.evaluate(ctx=2) reads 'child1'", !!(c1R as any).error === false && (c1R as any).value === "child1", JSON.stringify(c1R));

    // child2
    const c2R = await race(evalIn("document.getElementById('t').textContent", 3), 5000);
    step("Runtime.evaluate(ctx=3) reads 'child2'", !!(c2R as any).error === false && (c2R as any).value === "child2", JSON.stringify(c2R));

    return finish();
  } catch (e: any) { console.error("exception", e); return finish(); }
});

function finish() {
  console.log(`\n=== nested-iframe e2e: ${pass} PASS / ${fail} FAIL ===`);
  process.exit(fail === 0 ? 0 : 1);
}
