/**
 * W1b — JS-fetch interception CDP round-trip e2e.
 * Verifies: page fetch() -> Fetch.requestPaused -> Fetch.fulfillRequest ->
 *           the fetch promise resolves to the fulfilled mock body.
 *
 * Self-contained: starts its own mock server (page + endpoint), then drives CDP.
 *   bun acceptance/fetch-intercept.ts <cdp_port> [mock_port]
 *
 * Implements the path left unverified by e031352 (unit-tested, never e2e'd).
 */
const CDP = Number(process.argv[2] ?? 19321);
const MOCK = Number(process.argv[3] ?? 18091);
const ORIGIN = `http://127.0.0.1:${MOCK}`;

const PAGE = `<!DOCTYPE html><html><body><script>
window.__res = "pending";
fetch(${JSON.stringify(ORIGIN + "/api/intercept")})
  .then(function (r) { return r.text(); })
  .then(function (t) { window.__res = t; })
  .catch(function (e) { window.__res = "err:" + e; });
</script></body></html>`;

// The mock endpoint returns "REAL-BODY", but the test intercepts and fulfills
// with "INTERCEPTED-BODY" — proving interception overrides the wire response.
const mock = Bun.serve({
  port: MOCK,
  fetch(req) {
    const u = new URL(req.url);
    if (u.pathname === "/api/intercept") return new Response("REAL-BODY", { status: 200 });
    return new Response(PAGE, { headers: { "content-type": "text/html; charset=utf-8" } });
  },
});

let pass = 0, fail = 0;
function step(name: string, ok: boolean, detail = "") {
  pass += ok ? 1 : 0; fail += ok ? 0 : 1;
  console.log(`${ok ? "PASS" : "FAIL"}  ${name}${detail ? "  — " + detail : ""}`);
}

const ws = new WebSocket(`ws://127.0.0.1:${CDP}/ws`);
let nextId = 1;
const pending = new Map<number, (v: any) => void>();
let sid: string | undefined;
let pausedRequestId: string | undefined;

function send(method: string, params: any = {}): Promise<any> {
  const { promise, resolve } = Promise.withResolvers<any>();
  const id = nextId++; pending.set(id, resolve);
  const m: any = { id, method, params }; if (sid) m.sessionId = sid; ws.send(JSON.stringify(m)); return promise;
}
ws.addEventListener("message", (ev) => {
  const msg = JSON.parse((ev as MessageEvent).data as string);
  if (msg.id != null && pending.has(msg.id)) { pending.get(msg.id)!(msg); pending.delete(msg.id); }
  if (msg.method === "Target.attachedToTarget" && msg.params?.sessionId) sid = msg.params.sessionId;
  if (msg.method === "Fetch.requestPaused" && msg.params?.requestId) pausedRequestId = msg.params.requestId;
});
async function ev(expr: string) {
  const r = await send("Runtime.evaluate", { expression: expr, returnByValue: true });
  return r.result?.result?.value;
}

ws.addEventListener("open", async () => {
  try {
    await send("Target.setDiscoverTargets", { discover: true });
    await send("Target.setAutoAttach", { autoAttach: true, flatten: true });
    for (let i = 0; i < 100 && !sid; i++) await new Promise((r) => setTimeout(r, 50));
    step("CDP attach", !!sid);
    await send("Runtime.enable");
    await send("Page.enable");
    // Enable interception on the JS-fetch path with a URL pattern.
    await send("Fetch.enable", { patterns: [{ urlPattern: "*api/intercept*" }] });

    await send("Page.navigate", { url: ORIGIN + "/" });

    // Wait for Fetch.requestPaused to fire.
    for (let i = 0; i < 60 && !pausedRequestId; i++) await new Promise((r) => setTimeout(r, 100));
    step("Fetch.requestPaused emitted", !!pausedRequestId);

    if (pausedRequestId) {
      const body = btoa("INTERCEPTED-BODY");
      const fr = await send("Fetch.fulfillRequest", {
        requestId: pausedRequestId,
        responseCode: 200,
        body,
      });
      step("Fetch.fulfillRequest accepted", !fr.error, fr.error?.message ?? "");

      // Wait for the fetch promise to resolve to the fulfilled body.
      let res: any;
      for (let i = 0; i < 60; i++) {
        res = await ev("window.__res");
        if (res && res !== "pending") break;
        await new Promise((r) => setTimeout(r, 100));
      }
      step("fetch resolved to intercepted body", res === "INTERCEPTED-BODY", `(got ${JSON.stringify(res)})`);
    }

    console.log(`\n=== JS-fetch interception e2e: ${pass} PASS / ${fail} FAIL ===`);
    mock.stop();
    ws.close();
    process.exit(fail > 0 ? 1 : 0);
  } catch (e) {
    console.error("ERROR:", e);
    mock.stop();
    process.exit(1);
  }
});
ws.addEventListener("error", () => { console.error("WS error"); process.exit(1); });
setTimeout(() => { console.error("timed out"); process.exit(1); }, 30000);
