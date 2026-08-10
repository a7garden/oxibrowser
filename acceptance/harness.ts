/**
 * OxiBrowser acceptance harness — drives the mock SPA end-to-end over CDP.
 *
 * Flow: navigate → click "Go to Login" → fill credentials → submit →
 *       wait for dashboard → wait for async data → screenshot.
 *
 * Exercises: page-script execution, DOM query/mutation, event listeners,
 * hash routing, async fetch, and dynamic rendering — the core automation loop.
 *
 *   bun acceptance/harness.ts <cdp_port> <mock_port>
 */
type CdpResult = { id?: number; error?: { message?: string }; result?: any };
type CdpEvent = { method?: string; params: any };

const CDP = Number(process.argv[2] ?? 9222);
const MOCK = Number(process.argv[3] ?? 18080);
const SPA = `http://127.0.0.1:${MOCK}/`;

let pass = 0, fail = 0;
const steps: { name: string; ok: boolean; ms: number; detail?: string }[] = [];
function step(name: string, ok: boolean, ms = 0, detail = "") {
  pass += ok ? 1 : 0; fail += ok ? 0 : 1;
  steps.push({ name, ok, ms, detail });
  console.log(`${ok ? "PASS" : "FAIL"}  ${name}${detail ? "  — " + detail : ""}${ms ? `  (${ms}ms)` : ""}`);
}

const ws = new WebSocket(`ws://127.0.0.1:${CDP}/ws`);
let nextId = 1;
const pending = new Map<number, (v: CdpResult) => void>();
let sessionId: string | undefined;

function send(method: string, params: Record<string, unknown> = {}): Promise<CdpResult> {
  const { promise, resolve } = Promise.withResolvers<CdpResult>();
  const id = nextId++;
  pending.set(id, resolve);
  const msg: Record<string, unknown> = { id, method, params };
  if (sessionId) msg.sessionId = sessionId;
  ws.send(JSON.stringify(msg));
  return promise;
}
function race(p: Promise<CdpResult>, ms: number): Promise<CdpResult | "TIMEOUT"> {
  return Promise.race([p, new Promise<"TIMEOUT">((r) => setTimeout(() => r("TIMEOUT"), ms))]);
}

ws.addEventListener("message", (ev) => {
  const msg = JSON.parse((ev as MessageEvent).data as string);
  if (msg.id != null && pending.has(msg.id)) { pending.get(msg.id)!(msg); pending.delete(msg.id); }
  if (msg.method === "Target.attachedToTarget" && msg.params?.sessionId) sessionId = msg.params.sessionId;
});

/** Runtime.evaluate returning a by-value JS primitive. */
async function evalJs(expression: string): Promise<{ value: any; error?: string }> {
  const r = await race(send("Runtime.evaluate", { expression, returnByValue: true }), 8000);
  if (r === "TIMEOUT") return { value: undefined, error: "timeout" };
  if (r.error) return { value: undefined, error: r.error.message };
  const ex = r.result?.exceptionDetails;
  if (ex) return { value: undefined, error: ex.exception?.description ?? ex.text };
  return { value: r.result?.result?.value };
}

/** Poll until a CSS selector exists in the DOM. */
async function waitFor(selector: string, timeoutMs = 8000): Promise<boolean> {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const { value } = await evalJs(`document.querySelector(${JSON.stringify(selector)}) !== null`);
    if (value === true) return true;
    await new Promise((r) => setTimeout(r, 150));
  }
  return false;
}

async function fill(selector: string, value: string): Promise<void> {
  await evalJs(`(() => { var e = document.querySelector(${JSON.stringify(selector)}); if (!e) return false; e.value = ${JSON.stringify(value)}; e.dispatchEvent(new Event('input', { bubbles: true })); return true; })()`);
}
async function clickSelector(selector: string): Promise<void> {
  await evalJs(`(document.querySelector(${JSON.stringify(selector)}) || {}).click ? document.querySelector(${JSON.stringify(selector)}).click() : false`);
}

ws.addEventListener("open", async () => {
  const t0 = Date.now();
  try {
    await send("Target.setDiscoverTargets", { discover: true });
    await send("Target.setAutoAttach", { autoAttach: true, flatten: true });
    for (let i = 0; i < 100 && !sessionId; i++) await new Promise((r) => setTimeout(r, 50));
    step("CDP attach (sessionId)", !!sessionId);
    if (!sessionId) throw new Error("no sessionId");

    await send("Runtime.enable");
    await send("Page.enable");

    // 1. Navigate to SPA landing.
    let s = Date.now();
    const nav = await race(send("Page.navigate", { url: SPA }), 10000);
    step("navigate to SPA", !(nav === "TIMEOUT" || nav.error), Date.now() - s);

    // 2. Landing rendered (page script executed, router painted #login-link).
    s = Date.now();
    const landing = await waitFor("#login-link", 10000);
    step("landing view rendered (#login-link)", landing, Date.now() - s);

    // 3. Click "Go to Login" → hash routing must fire the listener.
    s = Date.now();
    await clickSelector("#login-link");
    const loginForm = await waitFor("#username", 8000);
    step("hash route → login form (#username)", loginForm, Date.now() - s);

    // 4. Fill credentials.
    await fill("#username", "admin");
    await fill("#password", "secret");
    const filled = await evalJs(`document.querySelector('#username').value + ':' + document.querySelector('#password').value`);
    step("fill credentials", filled.value === "admin:secret", 0, `(got ${JSON.stringify(filled.value)})`);

    // 5. Submit → async POST /api/session → on 200 routes to #/dashboard.
    s = Date.now();
    await clickSelector("#submit");
    const dashboard = await waitFor("#dashboard", 12000);
    step("submit → dashboard rendered (#dashboard)", dashboard, Date.now() - s, dashboard ? "" : "(async fetch/routing failed)");

    // 6. Async dashboard data fetched + rendered.
    s = Date.now();
    const items = await waitFor("#dashboard-items li", 10000);
    let labels: any = undefined;
    if (items) {
      const lv = await evalJs(`Array.from(document.querySelectorAll('#dashboard-items li')).map(function (li) { return li.textContent; })`);
      labels = lv.value;
    }
    step("async data fetched + rendered", !!labels && labels.length === 3, Date.now() - s, `(got ${JSON.stringify(labels)})`);

    // 7. Screenshot.
    s = Date.now();
    const shot = await race(send("Page.captureScreenshot", { format: "png" }), 10000);
    if (shot !== "TIMEOUT" && shot.result?.data) {
      await Bun.write("acceptance/baseline.png", Buffer.from(shot.result.data, "base64"));
      step("screenshot captured", true, Date.now() - s);
    } else {
      step("screenshot captured", false, Date.now() - s, "(no data)");
    }

    const result = {
      pass, fail,
      total_ms: Date.now() - t0,
      cdp: CDP, mock: MOCK, spa: SPA,
      timestamp: new Date().toISOString(),
      steps,
    };
    await Bun.write("acceptance/result.json", JSON.stringify(result, null, 2) + "\n");

    console.log(`\n=== Acceptance: ${pass} PASS / ${fail} FAIL (${Date.now() - t0}ms) ===`);
    ws.close();
    process.exit(fail > 0 ? 1 : 0);
  } catch (e) {
    console.error("HARNESS ERROR:", e);
    await Bun.write("acceptance/result.json", JSON.stringify({ pass, fail, error: String(e), steps }, null, 2) + "\n");
    ws.close();
    process.exit(1);
  }
});

ws.addEventListener("error", () => { console.error("WS error"); process.exit(1); });
setTimeout(() => { console.error("harness timed out"); process.exit(1); }, 60000);
