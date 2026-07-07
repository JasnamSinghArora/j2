// Progressive enhancement for the J docs: syntax highlighting, heading
// anchors, and active-nav marking. The page works fine without it.
(function () {
  "use strict";

  // ---- J syntax highlighting ----
  var KW = new Set(["func","if","else","for","in","repeat","loop","do","give",
    "stop","skip","try","assert","class","global","extends","import","and","or","not"]);
  var LIT = new Set(["true","false","null","self"]);
  var TY = new Set(["int","float","text","bool","seq","map"]);

  function esc(s) {
    return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
  }
  function span(cls, t) { return '<span class="tok-' + cls + '">' + esc(t) + "</span>"; }

  function highlight(src) {
    var re = /(#[^\n]*)|("(?:[^"\\]|\\.)*")|(\b\d[\d_]*(?:\.\d[\d_]*)?(?:[eE][-+]?\d+)?\b)|([A-Za-z_]\w*)/g;
    var out = "", last = 0, m;
    while ((m = re.exec(src)) !== null) {
      out += esc(src.slice(last, m.index));
      last = re.lastIndex;
      if (m[1]) out += span("comment", m[1]);
      else if (m[2]) out += span("string", m[2]);
      else if (m[3]) out += span("number", m[3]);
      else {
        var w = m[4];
        if (KW.has(w) || LIT.has(w)) out += span("keyword", w);
        else if (TY.has(w)) out += span("type", w);
        else if (src[re.lastIndex] === "(") out += span("func", w);
        else out += esc(w);
      }
    }
    out += esc(src.slice(last));
    return out;
  }

  function highlightAll() {
    document.querySelectorAll("pre > code").forEach(function (code) {
      if (code.dataset.nohl !== undefined) return;
      code.innerHTML = highlight(code.textContent);
    });
  }

  // ---- heading anchors ----
  function slug(s) {
    return s.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "");
  }
  function addAnchors() {
    document.querySelectorAll(".content h2, .content h3").forEach(function (h) {
      if (!h.id) h.id = slug(h.textContent);
      var a = document.createElement("a");
      a.href = "#" + h.id;
      a.className = "anchor";
      a.textContent = "#";
      a.setAttribute("aria-hidden", "true");
      h.appendChild(a);
    });
  }

  // ---- active nav link ----
  function markActive() {
    var here = location.pathname.split("/").pop() || "index.html";
    document.querySelectorAll(".sidebar nav a").forEach(function (a) {
      var href = a.getAttribute("href");
      if (href === here || (here === "" && href === "index.html")) a.classList.add("active");
    });
  }

  document.addEventListener("DOMContentLoaded", function () {
    try { highlightAll(); } catch (e) {}
    addAnchors();
    markActive();
  });
})();
