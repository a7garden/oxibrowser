const PORT = Number(process.argv[2] ?? 18099);
const PAGE = `<!DOCTYPE html><html><body>
<h1>t</h1>
<script>
window.__hash = "none";
window.addEventListener("hashchange", function (ev) {
  window.__hash = ev.type + ":" + ev.newURL + ":" + ev.oldURL;
});
setTimeout(function () {
  window.location.hash = "#two";
}, 200);
</script>
</body></html>`;
Bun.serve({
  port: PORT,
  fetch(_req) {
    return new Response(PAGE, { headers: { "content-type": "text/html" } });
  },
});
console.log(`hashchange mock listening on http://127.0.0.1:${PORT}`);
