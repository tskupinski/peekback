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
    const classes = new Set();
    this.classList = {
      toggle(name, enabled) { if (enabled ?? !classes.has(name)) classes.add(name); else classes.delete(name); },
      contains(name) { return classes.has(name); },
    };
  }
  addEventListener(name, fn) { (this.listeners[name] ??= []).push(fn); }
  dispatch(name, values = {}) {
    const event = { preventDefault() {}, stopPropagation() {}, ...values };
    for (const fn of this.listeners[name] ?? []) fn(event);
  }
  replaceChildren(...children) { this.children = children; }
  append(...children) { this.children.push(...children); }
  setAttribute(name, value) { this.attributes[name] = value; }
  removeAttribute(name) { delete this.attributes[name]; }
  matches() { return false; }
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
  // Paragraph-only rendering fixture. Exercise comment handlers against source
  // ranges without depending on a browser's Markdown/layout implementation.
  Object.defineProperty(element("doc"), "innerHTML", { set(source) {
    let line = 0;
    this.children = source ? source.split("\n\n").map(text => {
      const block = new Element();
      const count = text.split("\n").length;
      block.dataset = { sourceLine: String(line), sourceEnd: String(line + count) };
      line += count + 1;
      return block;
    }) : [];
  } });
  const messages = [];
  const window = Object.assign(new Element(), {
    matchMedia: () => ({ matches: false, addEventListener() {} }),
    markdownit: () => ({ use() { return this; }, render(source) { return source; } }),
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
  input.dispatch("keydown", { key: "Tab", shiftKey: true }); // Shift-Tab cycles backwards.
  receive({ type: "all-documents", documents, warnings: [] }); // Late reply must not change scope.
  assert.equal(element("picker-current").attributes["aria-pressed"], "true");
  assert.equal(element("picker-list").children[0].children[0].textContent, "current.md");
  element("picker-all").dispatch("click");
  receive({ type: "all-documents", documents: [], warnings: [] });
  assert.match(element("picker-list").children[0].textContent, /No existing Markdown/);
  input.dispatch("keydown", { key: "r", ctrlKey: true });
  assert.match(element("picker-note").textContent, /Loading/);
  console.log("Picker passed: scopes, async filtering, provenance search, warnings, >30 results, standalone opening, refresh.");

  element("picker-bookmarks").dispatch("click");
  assert.deepEqual(messages.at(-1), { type: "list-bookmarks" });
  assert.equal(element("picker-bookmarks").attributes["aria-pressed"], "true");
  assert.match(element("picker-note").textContent, /Loading bookmarks/);
  receive({ type: "bookmarks", documents: [], found: 0, warnings: [] });
  assert.match(element("picker-list").children[0].textContent, /No bookmarked Markdown files\. Add bookmarks = /);
  const marks = [
    { path: "/home/.claude/CLAUDE.md", label: "~/.claude/CLAUDE.md", touched_at: 1 },
    { path: "/project/CLAUDE.md", label: "CLAUDE.md", touched_at: 2 },
  ];
  input.dispatch("keydown", { key: "r", ctrlKey: true });
  assert.deepEqual(messages.at(-1), { type: "list-bookmarks" }); // Ctrl-R rereads the config.
  receive({ type: "bookmarks", documents: marks, found: 250, warnings: ["docs/[.md: unclosed character class"] });
  assert.deepEqual(element("picker-list").children.map(li => li.children[0].textContent), ["~/.claude/CLAUDE.md", "CLAUDE.md"]);
  assert.match(element("picker-note").textContent, /Showing 2 of 250 bookmarked files\. Some bookmarks could not be listed:\ndocs\/\[\.md/);
  input.dispatch("keydown", { key: "Tab" });
  assert.equal(element("picker-current").attributes["aria-pressed"], "true"); // Tab wraps around.
  input.dispatch("keydown", { key: "Tab", shiftKey: true });
  input.value = "project";
  input.dispatch("input");
  input.dispatch("keydown", { key: "Enter" });
  assert.deepEqual(messages.at(-1), { type: "open-bookmark", path: "/project/CLAUDE.md" });
  window.dispatch("keydown", { key: "p", ctrlKey: true });
  assert.deepEqual(messages.at(-1), { type: "list-bookmarks" }); // Reopening keeps the scope and refreshes.
  input.dispatch("keydown", { key: "Escape" });
  console.log("Bookmarks passed: scope cycling, config hint, cap and warnings, refresh, opening.");

  input.dispatch("keydown", { key: "Escape" });
  const key = key => window.dispatch("keydown", { key });
  const command = value => {
    key(":");
    element("cmd-input").value = value;
    element("cmd-input").dispatch("keydown", { key: "Enter" });
  };
  const source = "First paragraph.\n\nSecond paragraph.\n\nThird paragraph.";
  const render = async (path = "/comments.md", text = source) => {
    receive({ type: "render", path, source: text, documents: [] });
    await new Promise(setImmediate);
  };
  const marked = () => element("doc").children.filter(el => el.classList.contains("commented"));
  await render();
  key("v"); key("j"); command("c Review both paragraphs");
  assert.equal(marked().length, 2);
  assert.equal(marked()[0].dataset.commentLabel, "1 comment");
  assert.equal(marked()[0].attributes.title, "Review both paragraphs");
  command("c Another note");
  assert.equal(marked()[1].dataset.commentLabel, "2 comments");
  await render("/other.md");
  assert.equal(marked().length, 0);
  await render();
  assert.equal(marked().length, 2);
  await render("/comments.md", "New paragraph.\n\n" + source);
  assert.equal(marked().length, 0); // Never mark unrelated text after edits.
  await render();
  command("sendall");
  receive({ type: "send-result", purpose: "comments", ok: false, text: "failed" });
  assert.equal(marked().length, 2);
  command("sendall");
  command("c Added while sending");
  receive({ type: "send-result", purpose: "comments", ok: true, text: "sent" });
  assert.equal(marked().length, 1); // Only the acknowledged batch disappears.
  assert.equal(marked()[0].attributes.title, "Added while sending");
  element("comments").children[0].children[2].dispatch("click");
  assert.equal(marked().length, 0);
  assert.equal(element("doc").children[1].attributes.title, undefined);

  key("g"); key("g"); key("c");
  await render("/other.md");
  element("cmd-input").value = "Keep the original target";
  element("cmd-input").dispatch("keydown", { key: "Enter" });
  assert.equal(marked().length, 0);
  await render();
  assert.equal(marked()[0].attributes.title, "Keep the original target");
  console.log("Comments passed: ranges, overlapping notes, document switches, stale anchors, removal, send lifecycle, captured targets.");

  const anchor = (attributes, xlink = {}) => ({
    getAttribute: name => attributes[name] ?? null,
    getAttributeNS: (ns, name) => ns === "http://www.w3.org/1999/xlink" ? xlink[name] ?? null : null,
  });
  const click = link => {
    let prevented = false;
    document.dispatch("click", { target: { closest: () => link }, preventDefault() { prevented = true; } });
    return prevented;
  };
  const sent = messages.length;
  assert.equal(click(anchor({}, { href: "https://example.com/" })), true); // Mermaid's SVG links.
  assert.deepEqual(messages.at(-1), { type: "open-external", url: "https://example.com/" });
  assert.equal(click(anchor({ href: "javascript:alert(1)" })), true);
  assert.equal(click(anchor({}, { href: "file:///etc/passwd" })), true);
  assert.equal(messages.length, sent + 1);
  assert.equal(click(anchor({ href: "#section" })), false);
  assert.equal(click(anchor({})), false);
  console.log("Links passed: HTML and SVG anchors never navigate the webview.");

  const posted = messages.length;
  receive({ type: "render", path: null, source: "", session: { session_id: "fresh" }, documents: [] });
  await new Promise(setImmediate);
  assert.equal(element("no-documents").hidden, false);
  assert.equal(element("doc").children.length, 0);
  key("y"); key("s"); key("c"); key("]"); key("d");
  assert.equal(messages.length, posted); // Nothing to copy, send, comment on or cycle to.
  window.dispatch("keydown", { key: "p", ctrlKey: true });
  element("picker-current").dispatch("click");
  assert.match(element("picker-list").children[0].textContent, /has not written any Markdown yet/);
  input.dispatch("keydown", { key: "Escape" });
  receive({ type: "render", path: "/first.md", source: "First.", session: { session_id: "fresh" },
    documents: [{ path: "/first.md", label: "first.md", touched_at: 2 },
      { path: "/shell.md", label: "shell.md", touched_at: 1, scanned: true, shared: false },
      { path: "/shared.md", label: "shared.md", touched_at: 1, scanned: true, shared: true }] });
  await new Promise(setImmediate);
  assert.equal(element("no-documents").hidden, true);
  window.dispatch("keydown", { key: "p", ctrlKey: true });
  const metas = element("picker-list").children.map(li => li.children[1].textContent);
  assert.doesNotMatch(metas[0], /scan/);
  assert.match(metas[1], /· found by scan$/);
  assert.match(metas[2], /maybe another session's$/);
  input.dispatch("keydown", { key: "Escape" });
  console.log("Empty session passed: placeholder, inert actions, picker hint, first document replaces it, scan notes.");
}

main().catch(error => { console.error(error); process.exitCode = 1; });
