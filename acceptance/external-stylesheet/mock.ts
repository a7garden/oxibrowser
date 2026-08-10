const PORT = Number(process.argv[2] ?? 18096);
const MAIN = `<!DOCTYPE html><html><head>
  <link rel="stylesheet" href="/style.css">
</head><body>
  <p id="t" class="green">probe</p>
</body></html>`;
const CSS = `p.green { color: #008000 !important; }`;
Bun.serve({
  port: PORT,
  fetch(req) {
    const u = new URL(req.url);
    if (u.pathname === "/") return new Response(MAIN, { headers: { "content-type": "text/html" } });
    if (u.pathname === "/style.css") return new Response(CSS, { headers: { "content-type": "text/css" } });
    return new Response("not found", { status: 404 });
  },
});
console.log(`external-stylesheet mock listening on http://127.0.0.1:${PORT}`);
