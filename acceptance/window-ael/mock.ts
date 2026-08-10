const PORT = Number(process.argv[2] ?? 18097);
const PAGE = `<!DOCTYPE html><html><body>
<h1 id="t">init</h1>
<script>
window.addEventListener('load', () => {
  window.__loadFired = true;
});
</script>
</body></html>`;
Bun.serve({
  port: PORT,
  fetch(_req) {
    return new Response(PAGE, { headers: { "content-type": "text/html" } });
  },
});
console.log(`window.addEventListener mock listening on http://127.0.0.1:${PORT}`);
