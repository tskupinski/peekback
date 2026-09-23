// Exercise the real frontend event handlers/IPC with a minimal DOM adapter.
// Layout and native WebView rendering remain manual acceptance checks.
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const vm = require("node:vm");

class Element {
  constructor() {
    this.children = [];
    this.listeners = {};
    this.dataset = {};
    this.attributes = {};
    this.hidden = true;
    this.value = "";
    this.className = "";
    this.classList = { toggle() {}, contains() { return false; } };
  }
  addEventListener(name, fn) { (this.listeners[name] ??= []).push(fn); }
  dispatch(name, values = {}) {
    const event = { preventDefault() {}, stopPropagation() {}, ...values };
    for (const fn of this.listeners[name] ?? []) fn(event);
  }
  replaceChildren(...children) { this.children = children; }
  append(...children) { this.children.push(...children); }
  setAttribute(name, value) { this.attributes[name] = value; }
  querySelector(selector) { return selector === ".active" ? this.children.find(c => c.className.includes(" active")) : null; }
  querySelectorAll() { return []; }
  focus() {}
  blur() {}
  scrollIntoView() {}
}

async function main() {
  const nodes = new Map();
  const element = id => {
    if (!nodes.has(id)) nodes.set(id, new Element());
    return nodes.get(id);
  };
  const document = Object.assign(new Element(), {
    getElementById: element,
    createElement: () => new Element(),
    body: new Element(), documentElement: new Element(), hasFocus: () => true,
  });
  const messages = [];
  const window = Object.assign(new Element(), {
    matchMedia: () => ({ matches: false, addEventListener() {} }),
    markdownit: () => ({ use() { return this; }, render() { return ""; } }),
    ipc: { postMessage: text => messages.push(JSON.parse(text)) },
    scrollTo() {},
  });
  vm.runInNewContext(fs.readFileSync(path.join(__dirname, "../assets/app.js"), "utf8"), {
    window, document, console, CSS: {}, mermaid: { initialize() {} }, renderMathInElement() {},
    localStorage: { getItem() { return null; }, setItem() {} },
    setInterval() {}, setTimeout() {}, clearTimeout() {},
  });
  const receive = window.peekback.receive;
  receive({ type: "render", path: "/current.md", source: "", session: { session_id: "live" },
    documents: [{ path: "/current.md", label: "current.md", touched_at: 1 }] });
  await new Promise(setImmediate);
  window.dispatch("keydown", { key: "p", ctrlKey: true });
  assert.equal(element("picker-current").attributes["aria-pressed"], "true");
  assert.equal(element("picker-list").children[0].children[0].textContent, "current.md");
  const input = element("picker-input");
  input.dispatch("keydown", { key: "Tab" });
  assert.equal(messages.at(-1).type, "list-all-documents");
  assert.match(element("picker-note").textContent, /Loading/);
  input.value = "ended-session";
  input.dispatch("input");
  const documents = Array.from({ length: 45 }, (_, index) => ({
    path: `/project-${index}/note.md`, label: `/project-${index}/note.md`, touched_at: index + 1,
    sessions: [{ agent: "codex", session_id: "ended-session" }],
  }));
  receive({ type: "all-documents", documents, warnings: ["one damaged batch"] });
  assert.equal(input.value, "ended-session"); // Keep input typed while loading.
  assert.match(element("picker-note").textContent, /one damaged batch/);
  assert.equal(element("picker-list").children.length, 30);
  for (let i = 0; i < 35; i++) input.dispatch("keydown", { key: "ArrowDown" });
  const active = element("picker-list").querySelector(".active");
  assert.equal(active.children[0].textContent, "/project-35/note.md");
  input.dispatch("keydown", { key: "Enter" });
  assert.deepEqual(messages.at(-1), { type: "switch-tracked", path: "/project-35/note.md" });
  assert.equal(element("picker").hidden, true);

  window.dispatch("keydown", { key: "p", ctrlKey: true });
  assert.equal(messages.at(-1).type, "list-all-documents"); // Refresh on reopen.
  input.dispatch("keydown", { key: "Tab" });
  receive({ type: "all-documents", documents, warnings: [] }); // Late reply must not change scope.
  assert.equal(element("picker-current").attributes["aria-pressed"], "true");
  assert.equal(element("picker-list").children[0].children[0].textContent, "current.md");
  element("picker-all").dispatch("click");
  receive({ type: "all-documents", documents: [], warnings: [] });
  assert.match(element("picker-list").children[0].textContent, /No existing Markdown/);
  input.dispatch("keydown", { key: "r", ctrlKey: true });
  assert.match(element("picker-note").textContent, /Loading/);
  console.log("Picker passed: scopes, async filtering, provenance search, warnings, >30 results, standalone opening, refresh.");
}

main().catch(error => { console.error(error); process.exitCode = 1; });
