// OxiBrowser acceptance SPA — client-side logic.
// Exercises the automation loop oxibrowser supports end-to-end:
//   page-script execution, DOM render, element click listeners, async fetch,
//   dynamic re-render, error paths.
//
// Design notes (oxibrowser gaps worked around):
//  - Routing uses direct view calls from click handlers, NOT hashchange
//    (oxibrowser fires no hashchange; window.addEventListener is absent).
//  - fetch() uses ABSOLUTE URLs (oxibrowser does not resolve relative URLs
//    against the document base — throws "invalid URL"). The origin is injected
//    into window.__ORIGIN__ by server.ts.
"use strict";

var API = window.__ORIGIN__ || "";

function el(tag, attrs, children) {
  var n = document.createElement(tag);
  if (attrs) {
    for (var k in attrs) {
      if (k === "class") n.className = attrs[k];
      else if (k === "text") n.textContent = attrs[k];
      else n.setAttribute(k, attrs[k]);
    }
  }
  (children || []).forEach(function (c) { n.appendChild(c); });
  return n;
}

function view() { return document.getElementById("view"); }

function showLanding() {
  var v = view();
  v.innerHTML = "";
  v.appendChild(el("p", { text: "Welcome. Sign in to continue." }));
  var btn = el("button", { id: "login-link", text: "Go to Login" });
  btn.addEventListener("click", showLogin);
  v.appendChild(btn);
}

function showLogin() {
  var v = view();
  v.innerHTML = "";
  v.appendChild(el("h2", { text: "Sign in" }));
  v.appendChild(el("input", { id: "username", type: "text", placeholder: "username" }));
  v.appendChild(el("input", { id: "password", type: "password", placeholder: "password" }));
  v.appendChild(el("button", { id: "submit", text: "Submit" }));
  v.appendChild(el("div", { id: "error" }));

  document.getElementById("submit").addEventListener("click", function () {
    var user = document.getElementById("username").value;
    var pass = document.getElementById("password").value;
    var err = document.getElementById("error");
    err.textContent = "";
    var btn = document.getElementById("submit");
    btn.disabled = true;
    btn.textContent = "Signing in…";
    fetch(API + "/api/session", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ user: user, pass: pass })
    }).then(function (r) {
      window.__lastStatus = r.status;
      return r.text();
    }).then(function () {
      btn.disabled = false;
      btn.textContent = "Submit";
      if (window.__lastStatus === 200) {
        showDashboard();
      } else {
        err.textContent = "Invalid credentials (use admin / secret)";
      }
    }).catch(function (e) {
      btn.disabled = false;
      btn.textContent = "Submit";
      err.textContent = "Network error: " + e;
    });
  });
}

function showDashboard() {
  var v = view();
  v.innerHTML = "";
  v.appendChild(el("h2", { id: "dashboard", text: "Dashboard" }));
  v.appendChild(el("p", { class: "muted", text: "Loading data…" }));
  var list = el("ul", { id: "dashboard-items" });
  v.appendChild(list);

  fetch(API + "/api/data")
    .then(function (r) { return r.json(); })
    .then(function (items) {
      items.forEach(function (it) { list.appendChild(el("li", { text: it.label })); });
      window.__dashboardReady = true;
    })
    .catch(function (e) {
      v.appendChild(el("p", { text: "Failed to load data: " + e }));
    });
}

// Initial paint.
showLanding();
