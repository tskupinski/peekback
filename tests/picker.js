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
    const props = {};
    this.style = { setProperty(k, v) { props[k] = v; }, removeProperty(k) { delete props[k]; }, get: k => props[k] };
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
  getBoundingClientRect() { return { top: 0, bottom: 20, left: 10, right: 600 }; }
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
  const schemeListeners = [];
  let systemDark = false;
  let mermaidInits = 0;
  let mermaidOptions = null;
  let themeTokens = {};
  const window = Object.assign(new Element(), {
    matchMedia: () => ({ get matches() { return systemDark; }, addEventListener(_, fn) { schemeListeners.push(fn); } }),
    markdownit: () => ({ use() { return this; }, render(source) { return source; } }),
    ipc: { postMessage: text => messages.push(JSON.parse(text)) },
    scrollTo() {},
    scrollBy() {},
  });
  element("no-documents").textContent = "This session has not written any Markdown yet. Its first document opens here.";
  vm.runInNewContext(fs.readFileSync(path.join(__dirname, "../assets/app.js"), "utf8"), {
    window, document, console, CSS: {}, TextEncoder, mermaid: { initialize(options) { mermaidInits += 1; mermaidOptions = options; } }, renderMathInElement() {},
    localStorage: { getItem() { return null; }, setItem() {} },
    setInterval() {}, setTimeout() {}, clearTimeout() {}, requestAnimationFrame() {},
    getComputedStyle: () => ({ getPropertyValue: k => themeTokens[k.slice(2)] ?? "", colorScheme: systemDark ? "dark" : "light" }),
  });
  let context = { generation: 0, session: null };
  const receive = message => {
    // The daemon renders a live session with its cwd and display name.
    if (message.session) message.session = { cwd: "/", agent_name: "Claude Code", ...message.session };
    if (message.type === "render") {
      context = message.context ?? { generation: context.generation + 1,
        session: message.session ? { key: { agent: "claude", session_id: message.session.session_id }, incarnation: "test", process: null, started_at: 0 } : null };
    }
    const requestType = { scratchpad: "list-scratchpad", bookmarks: "list-bookmarks", "send-result": "send" }[message.type];
    const request_id = messages.findLast(m => m.type === requestType)?.request_id;
    window.peekback.receive({ context, request_id, outcome: "pasted", ...message });
  };
  const expectRequest = expected => {
    assert.equal(typeof messages.at(-1).request_id, "number");
    assert.deepEqual(messages.at(-1), { context, request_id: messages.at(-1).request_id, ...expected });
  };
  receive({ type: "render", path: "/current.md", source: "", session: { session_id: "live" },
    documents: [{ path: "/current.md", label: "current.md", touched_at: 1 }] });
  await new Promise(setImmediate);
  window.dispatch("keydown", { key: "p", ctrlKey: true });
  assert.equal(element("picker-current").attributes["aria-pressed"], "true");
  assert.equal(element("picker-list").children[0].children[0].textContent, "current.md");
  const input = element("picker-input");
  input.dispatch("keydown", { key: "Tab" });
  expectRequest({ type: "list-scratchpad" });
  assert.equal(element("picker-scratchpad").attributes["aria-pressed"], "true");
  assert.match(element("picker-note").textContent, /Loading scratchpad/);
  input.value = "note-4";
  input.dispatch("input");
  const notes = Array.from({ length: 45 }, (_, index) => ({
    path: `/scratch/note-${index}.md`, label: `note-${index}.md`, touched_at: 100 - index,
  }));
  receive({ type: "scratchpad", available: true, documents: notes, found: 250, warnings: [] });
  assert.equal(input.value, "note-4"); // Keep input typed while loading.
  assert.match(element("picker-note").textContent, /Showing 45 of 250 files\. Markdown in this session's scratchpad/);
  input.value = "";
  input.dispatch("input");
  assert.equal(element("picker-list").children.length, 30);
  for (let i = 0; i < 35; i++) input.dispatch("keydown", { key: "ArrowDown" });
  const active = element("picker-list").querySelector(".active");
  assert.equal(active.children[0].textContent, "note-35.md");
  input.dispatch("keydown", { key: "Enter" });
  expectRequest({ type: "open-scratchpad", path: "/scratch/note-35.md" });
  assert.equal(element("picker").hidden, true);

  window.dispatch("keydown", { key: "p", ctrlKey: true });
  expectRequest({ type: "list-scratchpad" }); // Refresh on reopen.
  assert.equal(element("picker-list").children[0].textContent, "loading…"); // Never the previous session's list.
  input.dispatch("keydown", { key: "Tab", shiftKey: true }); // Shift-Tab cycles backwards.
  receive({ type: "scratchpad", available: true, documents: notes, found: 45, warnings: [] }); // Late reply keeps scope.
  assert.equal(element("picker-current").attributes["aria-pressed"], "true");
  assert.equal(element("picker-list").children[0].children[0].textContent, "current.md");
  element("picker-scratchpad").dispatch("click");
  receive({ type: "scratchpad", available: true, documents: [], found: 0, warnings: [] });
  assert.match(element("picker-list").children[0].textContent, /No Markdown in this session's scratchpad yet/);
  receive({ type: "scratchpad", available: false, documents: [], found: 0, warnings: [] });
  assert.match(element("picker-list").children[0].textContent, /No scratchpad for this session/);
  assert.match(element("picker-note").textContent, /has ended, or it is not a Claude Code session/);
  input.dispatch("keydown", { key: "r", ctrlKey: true });
  assert.match(element("picker-note").textContent, /Loading/);
  console.log("Picker passed: scopes, scratchpad listing, filtering, caps, >30 results, opening, refresh, no scratchpad.");

  element("picker-bookmarks").dispatch("click");
  expectRequest({ type: "list-bookmarks" });
  assert.equal(element("picker-bookmarks").attributes["aria-pressed"], "true");
  assert.match(element("picker-note").textContent, /Loading bookmarks/);
  receive({ type: "bookmarks", documents: [], found: 0, warnings: [] });
  assert.match(element("picker-list").children[0].textContent, /No bookmarked Markdown files\. Add bookmarks = /);
  const marks = [
    { path: "/home/.claude/CLAUDE.md", label: "~/.claude/CLAUDE.md", touched_at: 1 },
    { path: "/project/CLAUDE.md", label: "CLAUDE.md", touched_at: 2 },
  ];
  input.dispatch("keydown", { key: "r", ctrlKey: true });
  expectRequest({ type: "list-bookmarks" }); // Ctrl-R rereads the config.
  receive({ type: "bookmarks", documents: marks, found: 250, warnings: ["docs/[.md: unclosed character class"] });
  assert.deepEqual(element("picker-list").children.map(li => li.children[0].textContent), ["~/.claude/CLAUDE.md", "CLAUDE.md"]);
  assert.match(element("picker-note").textContent, /Showing 2 of 250 bookmarked files\. Some bookmarks could not be listed:\ndocs\/\[\.md/);
  // A live reload of the viewed file is a new view; the open listing is asked for again.
  receive({ type: "render", path: "/current.md", source: "Edited", session: { session_id: "live" },
    documents: [{ path: "/current.md", label: "current.md", touched_at: 1 }] });
  expectRequest({ type: "list-bookmarks" });
  receive({ type: "bookmarks", documents: marks, found: 250, warnings: ["docs/[.md: unclosed character class"] });
  await new Promise(setImmediate);
  input.dispatch("keydown", { key: "Tab" });
  assert.equal(element("picker-current").attributes["aria-pressed"], "true"); // Tab wraps around.
  input.dispatch("keydown", { key: "Tab", shiftKey: true });
  receive({ type: "bookmarks", documents: marks, found: 250, warnings: [] });
  input.value = "project";
  input.dispatch("input");
  input.dispatch("keydown", { key: "Enter" });
  expectRequest({ type: "open-bookmark", path: "/project/CLAUDE.md" });
  window.dispatch("keydown", { key: "p", ctrlKey: true });
  expectRequest({ type: "list-bookmarks" }); // Reopening keeps the scope and refreshes.
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
    receive({ type: "render", path, source: text, session: { session_id: "reviewer" }, documents: [] });
    await new Promise(setImmediate);
  };
  // S opens the review sheet; Enter in it sends. An empty opening line keeps
  // these tests' prompts to the comments themselves.
  const sendAll = () => {
    command("sendall");
    element("review-intro").value = "";
    element("review-intro").dispatch("input");
    element("review-intro").dispatch("keydown", { key: "Enter" });
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
  assert.equal(marked().length, 2); // Moved text keeps its comments.
  assert.equal(marked()[0].dataset.sourceLine, "2");
  await render("/comments.md", source.replace("First paragraph.", "First, rewritten."));
  assert.equal(marked().length, 1); // Changed text is reported, never guessed at.
  assert.equal(marked()[0].attributes.title, "Another note");
  assert.equal(element("comments").children[0].children[1].hidden, false);
  assert.match(element("comments").children[0].children[1].textContent, /text changed or removed/);
  sendAll();
  assert.match(messages.at(-1).text, /Review both paragraphs/);
  assert.match(messages.at(-1).text, /\(This text has since changed or been removed\.\)\nReview both paragraphs/);
  receive({ type: "send-result", purpose: "comments", ok: false, text: "failed" });
  await render();
  assert.equal(element("comments").children[0].children[1].hidden, true);
  sendAll();
  receive({ type: "send-result", purpose: "comments", ok: false, text: "failed" });
  assert.equal(marked().length, 2);
  sendAll();
  command("c Added while sending");
  receive({ type: "send-result", purpose: "comments", ok: true, text: "sent" });
  assert.equal(marked().length, 1); // Only the acknowledged batch disappears.
  assert.equal(marked()[0].attributes.title, "Added while sending");
  element("comments").children[0].children[3].dispatch("click");
  assert.equal(marked().length, 0);
  assert.equal(element("doc").children[1].attributes.title, undefined);

  // Writing a note happens in a box under the block, all from the keyboard.
  key("g"); key("g"); key("j"); key("c");
  assert.equal(element("mode").textContent, "COMMENT");
  assert.equal(element("composer").hidden, false);
  assert.equal(element("composer-quote").textContent, "Second paragraph.");
  const composer = element("composer-input");
  composer.value = "First line";
  composer.dispatch("keydown", { key: "Enter", shiftKey: true }); // Shift-Enter is a new line, not a save.
  assert.equal(element("composer").hidden, false);
  composer.value = "First line\nsecond line";
  composer.dispatch("keydown", { key: "Enter" });
  assert.equal(element("composer").hidden, true);
  assert.equal(element("mode").textContent, "NORMAL");
  assert.equal(marked()[0].attributes.title, "First line\nsecond line");
  key("c"); composer.value = "Never mind"; composer.dispatch("keydown", { key: "Escape" });
  assert.equal(element("composer").hidden, true);
  assert.equal(element("comments").children.length, 1); // Esc adds nothing.
  // C edits the comment on the cursor's block; Ctrl-E does it from the list.
  key("C");
  assert.equal(composer.value, "First line\nsecond line");
  composer.value = "Reworded";
  composer.dispatch("keydown", { key: "Enter" });
  assert.equal(marked()[0].attributes.title, "Reworded");
  assert.equal(element("comments").children.length, 1);
  key("g"); key("g"); key("C");
  assert.match(element("status-msg").textContent, /no comment on this block · c adds one/);
  key(" "); key("c");
  input.dispatch("keydown", { key: "e", ctrlKey: true });
  assert.equal(composer.value, "Reworded");
  composer.dispatch("keydown", { key: "Escape" });
  // What the agent gets: where each quote is, as a line reference.
  sendAll();
  assert.match(messages.at(-1).text, /Comments on `\/comments\.md`:\n\n`\/comments\.md#L3`\n> Second paragraph\.\nReworded$/);
  receive({ type: "send-result", purpose: "comments", ok: true, text: "sent" });
  assert.equal(element("comments").children.length, 0);

  // Send all reviews first: the sheet shows the target and exactly what will
  // be pasted, with an opening line you can change.
  command("c Review this");
  const intro = element("review-intro");
  const beforeReview = messages.length;
  key("S");
  assert.equal(element("mode").textContent, "REVIEW");
  assert.equal(element("review").hidden, false);
  assert.equal(element("review-target").textContent, "Send 1 comment to / · Claude Code");
  assert.equal(intro.value, "");  // The line last sent with; these tests sent without one.
  intro.value = "Address these review comments.";
  intro.dispatch("input");
  assert.match(element("review-preview").textContent, /^Address these review comments\.\n\nComments on `\/comments\.md`:\n\n`\/comments\.md#L1`\n> First paragraph\.\nReview this$/);
  intro.dispatch("keydown", { key: "ArrowDown" }); // Scrolls the preview, nothing else.
  intro.dispatch("keydown", { key: "Escape" });
  assert.equal(element("review").hidden, true);
  assert.equal(messages.length, beforeReview); // Closing sends nothing.
  key("S");
  intro.value = "Please fix:";
  intro.dispatch("input");
  intro.dispatch("keydown", { key: "Enter" });
  assert.match(messages.at(-1).text, /^Please fix:\n\nComments on/);
  receive({ type: "send-result", purpose: "comments", ok: true, text: "sent" });
  assert.equal(element("comments").children.length, 0);
  // Ctrl-S in the comments picker sends just the chosen one, through the sheet.
  command("c Keep"); key("j"); command("c Send only this");
  key(" "); key("c");
  input.value = "only this";
  input.dispatch("input");
  input.dispatch("keydown", { key: "s", ctrlKey: true });
  assert.equal(element("review-target").textContent, "Send 1 comment to / · Claude Code");
  assert.equal(intro.value, "Please fix:"); // Remembered.
  assert.doesNotMatch(element("review-preview").textContent, /Keep/);
  intro.dispatch("keydown", { key: "Enter" });
  receive({ type: "send-result", purpose: "comments", ok: true, text: "sent" });
  assert.deepEqual(element("comments").children.map(li => li.children[2].textContent), ["Keep"]);
  element("comments").children[0].children[3].dispatch("click");
  intro.value = "";
  key("S");
  assert.match(element("status-msg").textContent, /no pending comments for this session/);
  assert.equal(element("review").hidden, true);
  // Without a session there is nowhere to send: the sheet says so and Enter does nothing.
  receive({ type: "render", path: "/standalone.md", source, documents: [] });
  await new Promise(setImmediate);
  command("c Standalone note");
  const beforeStandalone = messages.length;
  key("S");
  assert.match(element("review-target").textContent, /No session to send to/);
  intro.dispatch("keydown", { key: "Enter" });
  assert.equal(messages.length, beforeStandalone);
  intro.dispatch("keydown", { key: "Escape" });
  element("comments").children[0].children[3].dispatch("click");
  await render();

  // One bar per comment beside its blocks: a range is one mark, not three, and
  // overlapping comments sit in lanes side by side.
  const bars = () => element("doc").children.find(el => el.className === "comment-gutter")?.children ?? [];
  key("g"); key("g"); key("v"); key("j"); key("j"); command("c Whole section");
  key("g"); key("g"); key("j"); command("c Just the second");
  assert.equal(bars().length, 2);
  assert.deepEqual(bars().map(b => b.style.left), ["20px", "24px"]);
  assert.equal(element("status-pending").hidden, false);
  assert.equal(element("status-pending").textContent, "2 pending · S send");
  assert.doesNotMatch(element("status-doc").textContent, /pending/);
  // ]c and [c read the notes without the mouse.
  key("g"); key("g"); key("]"); key("c"); // The next one, as in vim, not the one here.
  assert.match(element("status-msg").textContent, /^comment 2 of 2: Just the second$/);
  assert.equal(bars().find(b => b.className.includes("current")).title, "Just the second");
  key("]"); key("c");
  assert.match(element("status-msg").textContent, /^comment 1 of 2: Whole section$/); // Wraps around.
  key("["); key("c");
  assert.match(element("status-msg").textContent, /^comment 2 of 2/);
  sendAll();
  receive({ type: "send-result", purpose: "comments", ok: true, text: "sent" });
  assert.equal(bars().length, 0);
  assert.equal(element("status-pending").hidden, true);
  key("]"); key("c");
  assert.match(element("status-msg").textContent, /no comments in this document/);

  // Sending a selection takes the comments inside it along, and sends them.
  key("g"); key("g"); command("c On the first");
  key("G"); command("c On the last");
  key("g"); key("g"); key("v"); key("j"); key("s");
  const withComments = messages.at(-1);
  assert.equal(withComments.purpose, "comments");
  assert.match(withComments.text, /^`\/comments\.md#L1-3`\n> First paragraph\.\n>\n> Second paragraph\.\n\nComments on/);
  assert.match(withComments.text, /`\/comments\.md#L1`\n> First paragraph\.\nOn the first/);
  assert.match(withComments.text, /On the first/);
  assert.doesNotMatch(withComments.text, /On the last/);
  receive({ type: "send-result", purpose: "comments", ok: true, text: "sent" });
  assert.deepEqual(element("comments").children.map(li => li.children[2].textContent), ["On the last"]);
  element("comments").children[0].children[3].dispatch("click");
  key("Escape");
  // A comment reaching beyond the selection stays behind.
  key("g"); key("g"); key("v"); key("j"); command("c On both");
  key("g"); key("g"); key("s");
  assert.equal(messages.at(-1).purpose, "selection");
  assert.doesNotMatch(messages.at(-1).text, /On both/);
  receive({ type: "send-result", purpose: "selection", ok: true, text: "sent" });
  element("comments").children[0].children[3].dispatch("click");
  // With one of two identical blocks deleted, there is no telling which was
  // yours: the comment is reported, not moved to the other.
  const repeated = "Same.\n\nMiddle.\n\nSame.";
  await render("/repeated.md", repeated);
  key("G"); command("c On the second Same");
  await render("/repeated.md", "Same.\n\nMiddle.");
  assert.equal(marked().length, 0);
  assert.equal(element("comments").children[0].children[1].hidden, false);
  await render("/repeated.md", "Intro.\n\n" + repeated);
  assert.equal(marked().length, 1); // Both copies back: it follows the move.
  assert.equal(marked()[0].dataset.sourceLine, "6");
  element("comments").children[0].children[3].dispatch("click");
  await render();

  // Too large for the daemon: refused in the page, and not left "in flight".
  const huge = "x".repeat(4.5 * 1024 * 1024);
  await render("/huge.md", huge);
  command("c Too much");
  const beforeHuge = messages.length;
  sendAll();
  assert.equal(messages.length, beforeHuge);
  assert.match(element("toast").textContent, /Too large to send/);
  sendAll();
  assert.match(element("toast").textContent, /Too large to send/); // Not "still sending".
  element("comments").children[0].children[3].dispatch("click");
  await render();

  // Leaving the document saves the note with it; the box does not follow.
  key("g"); key("g"); key("c");
  element("composer-input").value = "Keep the original target";
  await render("/other.md");
  assert.equal(element("composer").hidden, true);
  assert.match(element("toast").textContent, /note saved with its document/);
  assert.equal(marked().length, 0);
  await render();
  assert.equal(marked()[0].attributes.title, "Keep the original target");
  key("g"); key("g"); key("c"); // An empty note is just closed.
  await render("/other.md");
  assert.equal(element("composer").hidden, true);
  await render();
  assert.equal(element("comments").children.length, 1);
  // A reload while writing a note on a range keeps the note open and on its
  // text, and no question interrupts it.
  element("comments").children[0].children[3].dispatch("click");
  key("g"); key("g"); key("v"); key("j"); key("c");
  element("composer-input").value = "Written during a reload";
  await render("/comments.md", "Inserted.\n\n" + source);
  assert.equal(element("mode").textContent, "COMMENT");
  assert.equal(element("composer").hidden, false);
  element("composer-input").dispatch("keydown", { key: "Enter" });
  assert.deepEqual(marked().map(b => b.dataset.sourceLine), ["2", "4"]); // Followed its text.
  element("comments").children.forEach(() => element("comments").children[0].children[3].dispatch("click"));
  await render();
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

  // A focused Codex pane before its first prompt: no session exists yet.
  receive({ type: "render", path: null, source: "", session: null, documents: [], unstarted: "Codex" });
  await new Promise(setImmediate);
  assert.equal(element("no-documents").hidden, false);
  assert.match(element("no-documents").textContent, /This Codex session starts when you send its first prompt/);

  const posted = messages.length;
  receive({ type: "render", path: null, source: "", session: { session_id: "fresh" }, documents: [] });
  await new Promise(setImmediate);
  assert.equal(element("no-documents").hidden, false);
  assert.match(element("no-documents").textContent, /has not written any Markdown yet/);
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

  const project = async (id, text, file = "README.md") => {
    receive({ type: "render", path: `/${id}/${file}`, file_identity: `/${id}/${file}`, source: text,
      session: { session_id: id, cwd: `/${id}` }, documents: [] });
    await new Promise(setImmediate);
  };
  await project("a", "Project A"); command("c Fix A");
  const aContext = context;
  // A resume is the same session: its comments stay.
  receive({ type: "render", path: "/a/README.md", file_identity: "/a/README.md", source: "Project A",
    session: { session_id: "a", cwd: "/a" }, documents: [],
    context: { generation: 99, session: { ...aContext.session, incarnation: "resumed" } } });
  await new Promise(setImmediate);
  assert.equal(element("comments").children.length, 1);
  // Another session parks them, without asking; coming back restores them.
  await project("b", "Project B");
  assert.equal(element("comments").children.length, 0);
  assert.doesNotMatch(element("status-doc").textContent, /pending/);
  await project("a", "Project A");
  assert.equal(element("comments").children.length, 1);
  assert.match(element("toast").textContent, /1 unsent comment restored/);
  await project("a", "Project A, reloaded");
  assert.doesNotMatch(element("toast").textContent, /Project A, reloaded/); // A reload restores nothing.
  // Parked comments wait however long it takes, through the session's agent
  // exiting and being resumed: coming back in a new incarnation finds them.
  await project("b", "Project B");
  receive({ type: "render", path: "/a/README.md", file_identity: "/a/README.md", source: "Project A",
    session: { session_id: "a", cwd: "/a" }, documents: [],
    context: { generation: 120, session: { ...aContext.session, incarnation: "resumed later" } } });
  await new Promise(setImmediate);
  assert.equal(element("comments").children.length, 1);
  element("comments").children[0].children[3].dispatch("click");
  // Every question is answered in turn, and only an answer ends one: a
  // reload does not hide "Pasted it?".
  command("c Fix A once");
  sendAll();
  receive({ type: "send-result", purpose: "comments", ok: true, outcome: "copied", text: "copied" });
  receive({ type: "render", path: "/a/README.md", file_identity: "/a/README.md", source: "Project A, edited",
    session: { session_id: "a", cwd: "/a" }, documents: [] });
  await new Promise(setImmediate);
  assert.equal(element("mode").textContent, "CONFIRM");
  assert.match(element("status-msg").textContent, /Copied 1 comment\. Pasted\? y marks sent · n keeps/);
  key("j");
  assert.equal(element("mode").textContent, "CONFIRM"); // Other keys do nothing.
  key("Escape"); // Escape keeps them.
  assert.equal(element("comments").children.length, 1);
  sendAll();
  receive({ type: "send-result", purpose: "comments", ok: true, outcome: "copied", text: "copied" });
  key("y");
  assert.equal(element("comments").children.length, 0);
  // A question arriving while a note is being written waits for it.
  command("c Fix A again");
  sendAll();
  key("c");
  element("composer-input").value = "Written while asked";
  receive({ type: "send-result", purpose: "comments", ok: true, outcome: "copied", text: "copied" });
  assert.equal(element("mode").textContent, "COMMENT");
  element("composer-input").dispatch("keydown", { key: "Enter" });
  await new Promise(setImmediate);
  assert.equal(element("mode").textContent, "CONFIRM");
  assert.match(element("status-msg").textContent, /Pasted\?/); // Saving the note did not replace it.
  key("y");
  assert.deepEqual(element("comments").children.map(li => li.children[2].textContent), ["Written while asked"]);
  // While "Pasted?" waits behind an open input, nothing more can be sent:
  // those comments may already be in the prompt.
  command("sendall"); // The sheet is open when the copied result arrives...
  element("review-intro").dispatch("keydown", { key: "Enter" });
  command("sendall");
  receive({ type: "send-result", purpose: "comments", ok: true, outcome: "copied", text: "copied" });
  assert.equal(element("mode").textContent, "REVIEW"); // ...so the question waits.
  const beforeBlocked = messages.length;
  element("review-intro").dispatch("keydown", { key: "Enter" });
  assert.equal(messages.length, beforeBlocked); // Not sent a second time.
  await new Promise(setImmediate);
  assert.equal(element("mode").textContent, "CONFIRM");
  key("y");
  assert.equal(element("comments").children.length, 0);
  // A comment removed while the sheet is open is not sent.
  command("c Stays"); key("j"); command("c Removed while reviewing");
  command("sendall");
  element("comments").children[1].children[3].dispatch("click");
  assert.doesNotMatch(element("review-preview").textContent, /Removed while reviewing/);
  element("review-intro").dispatch("keydown", { key: "Enter" });
  assert.doesNotMatch(messages.at(-1).text, /Removed while reviewing/);
  receive({ type: "send-result", purpose: "comments", ok: true, outcome: "pasted", text: "sent" });
  assert.equal(element("comments").children.length, 0);
  // The sidebar's Send all button sends like S does.
  command("c From the button");
  element("send-all").dispatch("click");
  assert.equal(element("mode").textContent, "REVIEW"); // The button reviews too.
  element("review-intro").dispatch("keydown", { key: "Enter" });
  assert.equal(messages.at(-1).type, "send");
  assert.match(messages.at(-1).text, /From the button/);
  receive({ type: "send-result", purpose: "comments", ok: true, outcome: "pasted", text: "sent" });
  assert.equal(element("comments").children.length, 0);
  await project("b", "Project B"); command("c Fix B"); sendAll();
  const bSend = messages.at(-1);
  assert.equal(bSend.type, "send");
  assert.equal(bSend.context.session.key.session_id, "b");
  assert.match(bSend.text, /Fix B/);
  assert.doesNotMatch(bSend.text, /Fix A/);
  assert.match(bSend.text, /(^|[^/])README\.md/, "paths are relative to the session's cwd");
  assert.doesNotMatch(bSend.text, /\/b\/README\.md/);
  assert.match(element("status-doc").textContent, /^b · Claude Code › /);
  assert.equal(element("comments").children.length, 1);
  receive({ type: "send-result", purpose: "comments", ok: true, request_id: bSend.request_id - 1, text: "stale" });
  assert.equal(element("comments").children.length, 1);
  receive({ type: "send-result", purpose: "comments", ok: true, outcome: "copied", text: "copied" });
  assert.equal(element("comments").children.length, 1); // Clipboard fallback keeps the draft until answered.
  key("y");
  assert.equal(element("comments").children.length, 0); // So it cannot go out twice.
  await project("b", "Project B"); command("c Fix B again"); sendAll();
  receive({ type: "send-result", purpose: "comments", ok: true, outcome: "pasted", text: "sent" });
  assert.equal(element("comments").children.length, 0);
  await project("a", "Project A"); command("c Fix A"); sendAll();
  assert.match(messages.at(-1).text, /Fix A/);
  assert.doesNotMatch(messages.at(-1).text, /Fix B/);
  receive({ type: "send-result", purpose: "comments", ok: false, text: "failed" });

  window.dispatch("keydown", { key: "p", ctrlKey: true });
  element("picker-bookmarks").dispatch("click");
  const oldList = messages.at(-1);
  input.dispatch("keydown", { key: "r", ctrlKey: true });
  const newList = messages.at(-1);
  receive({ type: "bookmarks", request_id: oldList.request_id, documents: marks, found: 2, warnings: [] });
  assert.match(element("picker-list").children[0].textContent, /loading/);
  receive({ type: "bookmarks", request_id: newList.request_id, context: aContext, documents: marks, found: 2, warnings: [] });
  assert.match(element("picker-list").children[0].textContent, /loading/);
  receive({ type: "bookmarks", request_id: newList.request_id, documents: marks, found: 2, warnings: [] });
  assert.equal(element("picker-list").children.length, 2);
  input.dispatch("keydown", { key: "Escape" });
  const beforeRender = messages.length;
  receive({ type: "render", path: "/b/README.md", source: "New source", session: { session_id: "b" }, documents: [] });
  key("s"); key("c");
  assert.equal(messages.length, beforeRender); // No action may use the previous render's block ranges.
  await new Promise(setImmediate);

  // Auto follows the system between the light and dark themes, and diagrams
  // follow along; a pinned theme ignores the system.
  const root = document.documentElement;
  receive({ type: "theme", theme: { name: "auto", font_family: "My Mono", font_size: 14 } });
  assert.equal(root.dataset.theme, "light");
  assert.equal(root.style.get("--mono"), '"My Mono", ui-monospace, Menlo, monospace');
  assert.equal(root.style.get("--mono-size"), "14px");
  const initsBefore = mermaidInits;
  systemDark = true;
  for (const fn of schemeListeners) fn({ matches: true });
  assert.equal(root.dataset.theme, "dark");
  assert.equal(mermaidInits, initsBefore + 1);
  receive({ type: "theme", theme: { name: "light", font_family: null, font_size: null } });
  assert.equal(root.dataset.theme, "light");
  assert.equal(root.style.get("--mono"), undefined);
  const pinnedInits = mermaidInits;
  for (const fn of schemeListeners) fn({ matches: true });
  assert.equal(root.dataset.theme, "light");
  assert.equal(mermaidInits, pinnedInits);
  systemDark = false;
  // Diagrams get plain colors: Mermaid cannot parse CSS color-mix().
  themeTokens = { bg: "#ffffff", fg: "#000000", accent: "#0000ff", annotate: "#ff0000", muted: "#555555", "code-bg": "#eeeeee" };
  receive({ type: "theme", theme: { name: "light", font_family: null, font_size: null } });
  assert.equal(mermaidOptions.theme, "base");
  assert.equal(mermaidOptions.themeVariables.primaryColor, "#d6d6ff");
  assert.equal(mermaidOptions.themeVariables.secondaryColor, "#ffd6d6");
  for (const value of Object.values(mermaidOptions.themeVariables)) assert.doesNotMatch(String(value), /color-mix/);
  themeTokens = {};
  console.log("Session isolation passed: drafts, delivery identity, acknowledgments, copied drafts, stale listings, rendering actions.");
  console.log("Themes applied: auto follows the system, pinned themes stay, font overrides.");
}

main().catch(error => { console.error(error); process.exitCode = 1; });
