/**
 * Mock server for the nested-iframe acceptance test.
 * main HTML embeds child1; child1 HTML embeds child2.
 */
const PORT = Number(process.argv[2] ?? 18093);

const MAIN = `<!DOCTYPE html><html><body>
<h1 id="t">main</h1>
<iframe id="child1" src="/child1"></iframe>
</body></html>`;

const CHILD1 = `<!DOCTYPE html><html><body>
<h2 id="t">child1</h2>
<iframe id="child2" src="/child2"></iframe>
</body></html>`;

const CHILD2 = `<!DOCTYPE html><html><body>
<h3 id="t">child2</h3>
<p id="deep">deepest</p>
</body></html>`;

Bun.serve({
  port: PORT,
  fetch(req) {
    const url = new URL(req.url);
    if (url.pathname === "/") return new Response(MAIN, { headers: { "content-type": "text/html" } });
    if (url.pathname === "/child1") return new Response(CHILD1, { headers: { "content-type": "text/html" } });
    if (url.pathname === "/child2") return new Response(CHILD2, { headers: { "content-type": "text/html" } });
    return new Response("not found", { status: 404 });
  },
});
console.log(`nested-iframe mock listening on http://127.0.0.1:${PORT}`);
