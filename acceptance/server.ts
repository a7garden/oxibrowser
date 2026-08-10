// OxiBrowser acceptance harness — mock SPA + API server.
//   bun acceptance/server.ts [port]
// Serves the static SPA from acceptance/www/ plus two JSON API endpoints.
// Injects the absolute origin into index.html (oxibrowser's JS fetch() does
// not resolve relative URLs against the document base).
import { file } from "bun";

const PORT = Number(Bun.env.MOCK_PORT ?? process.argv[2] ?? 18080);
const ROOT = new URL("./www/", import.meta.url);
const ORIGIN = `http://127.0.0.1:${PORT}`;

const indexHtml = (await file(new URL("index.html", ROOT)).text()).replaceAll("__OXI_ORIGIN__", ORIGIN);
const appJs = await file(new URL("app.js", ROOT)).text();
const styleCss = await file(new URL("style.css", ROOT)).text();

const server = Bun.serve({
  port: PORT,
  fetch(req) {
    const url = new URL(req.url);

    // --- API endpoints -----------------------------------------------------
    if (url.pathname === "/api/session" && req.method === "POST") {
      return req.json().then((body) => {
        if (body && body.user === "admin" && body.pass === "secret") {
          return Response.json({ token: "ok-" + Date.now() }, { status: 200 });
        }
        return Response.json({ error: "invalid credentials" }, { status: 401 });
      });
    }
    if (url.pathname === "/api/data" && req.method === "GET") {
      return Response.json([
        { id: 1, label: "Item alpha" },
        { id: 2, label: "Item beta" },
        { id: 3, label: "Item gamma" },
      ]);
    }

    // --- Static SPA --------------------------------------------------------
    const ct = { html: "text/html; charset=utf-8", js: "text/javascript", css: "text/css" };
    if (url.pathname === "/" || url.pathname === "/index.html")
      return new Response(indexHtml, { headers: { "content-type": ct.html } });
    if (url.pathname === "/app.js")
      return new Response(appJs, { headers: { "content-type": ct.js } });
    if (url.pathname === "/style.css")
      return new Response(styleCss, { headers: { "content-type": ct.css } });
    return new Response("not found", { status: 404 });
  },
});

console.log("acceptance server on " + server.port);
