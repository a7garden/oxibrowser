const PORT = Number(process.argv[2] ?? 18098);
const PAGE = `<!DOCTYPE html><html><body>
<h1>t</h1>
<script>
window.__res = "pending";
fetch("/api/items.json")
  .then(function (r) { return r.json(); })
  .then(function (j) { window.__res = JSON.stringify(j); })
  .catch(function (e) { window.__res = "err:" + e; });
</script>
</body></html>`;
Bun.serve({
  port: PORT,
  fetch(req) {
    const u = new URL(req.url);
    if (u.pathname === "/") return new Response(PAGE, { headers: { "content-type": "text/html" } });
    if (u.pathname === "/api/items.json") return new Response('["alpha","beta"]', { headers: { "content-type": "application/json" } });
    return new Response("not found", { status: 404 });
  },
});
console.log(`relative-fetch mock listening on http://127.0.0.1:${PORT}`);
