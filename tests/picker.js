// Exercise the real frontend event handlers/IPC with a minimal DOM adapter.
// Layout and native WebView rendering remain manual acceptance checks. Each
// scenario boots its own page, so one can fail or change without the others.
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
  replaceChildren(...children) { this.children = adopt(this, children); }
  append(...children) { this.children.push(...adopt(this, children)); }
  prepend(...children) { this.children.unshift(...adopt(this, children)); }
  replaceWith(other) {
    const siblings = this.parentElement.children;
    siblings[siblings.indexOf(this)] = adopt(this.parentElement, [other])[0];
  }
  setAttribute(name, value) { this.attributes[name] = value; }
  removeAttribute(name) { delete this.attributes[name]; }
  matches(selector) { return matchesSelector(this, selector); }
  closest(selector) {
    for (let node = this; node; node = node.parentElement) if (node.matches(selector)) return node;
    return null;
  }
  contains(other) {
    for (let node = other; node; node = node.parentElement) if (node === this) return true;
    return false;
  }
  querySelector(selector) { return selector === ".active" ? this.children.find(c => c.className.includes(" active")) : null; }
  querySelectorAll(selector) {
    const found = [];
    const walk = node => node.children.forEach(child => { if (matchesSelector(child, selector, this)) found.push(child); walk(child); });
    walk(this);
    return found;
  }
  focus() {}
  blur() {}
  scrollIntoView() {}
  // Blocks are laid out 20px per source line, so a bar's place says which
  // lines it marks.
  getBoundingClientRect() {
    return { top: Number(this.dataset.sourceLine ?? 0) * 20, bottom: Number(this.dataset.sourceEnd ?? 1) * 20, left: 10, right: 600 };
  }
}

const source = "First paragraph.\n\nSecond paragraph.\n\nThird paragraph.";
const marks = [
  { path: "/home/.claude/CLAUDE.md", label: "~/.claude/CLAUDE.md", touched_at: 1 },
  { path: "/project/CLAUDE.md", label: "CLAUDE.md", touched_at: 2 },
];

// A fresh page, and the keys, messages and renders a scenario drives it with.
// With markdown, documents go through the real markdown-it the page ships;
// otherwise each paragraph of the source is one block.
function boot({ markdown = false } = {}) {
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
    if (markdown) {
      this.children = adopt(this, parseHtml(source));
      return;
    }
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
  const env = { systemDark: false, mermaidInits: 0, mermaidOptions: null, themeTokens: {} };
  const window = Object.assign(new Element(), {
    matchMedia: () => ({ get matches() { return env.systemDark; }, addEventListener(_, fn) { schemeListeners.push(fn); } }),
    markdownit: markdown ? require("../assets/vendor/markdown-it.min.js")
      : () => ({ use() { return this; }, render(source) { return source; } }),
    markdownitFootnote: require("../assets/vendor/markdown-it-footnote.min.js"),
    markdownitTaskLists: require("../assets/vendor/markdown-it-task-lists.min.js"),
    ipc: { postMessage: text => messages.push(JSON.parse(text)) },
    scrollTo() {},
    scrollBy() {},
  });
  element("no-documents").textContent = "This session has not written any Markdown yet. Its first document opens here.";
  vm.runInNewContext(fs.readFileSync(path.join(__dirname, "../assets/app.js"), "utf8"), {
    window, document, console, CSS: {}, TextEncoder, hljs: { getLanguage: () => null },
    mermaid: { initialize(options) { env.mermaidInits += 1; env.mermaidOptions = options; } }, renderMathInElement() {},
    setInterval() {}, setTimeout() {}, clearTimeout() {}, requestAnimationFrame() {},
    getComputedStyle: () => ({ getPropertyValue: k => env.themeTokens[k.slice(2)] ?? "", colorScheme: env.systemDark ? "dark" : "light" }),
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
  const key = key => window.dispatch("keydown", { key });
  const command = value => {
    key(":");
    element("cmd-input").value = value;
    element("cmd-input").dispatch("keydown", { key: "Enter" });
  };
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
  const bars = () => element("doc").children.find(el => el.className === "comment-gutter")?.children ?? [];
  // The comments drawn in the margin: the source lines each bar spans, and its note.
  const marked = () => bars().map(bar => {
    const top = parseFloat(bar.style.top);
    return { line: top / 20, endLine: (top + parseFloat(bar.style.height)) / 20, note: bar.title };
  });
  // What the status line counts, and what the comments picker lists, as a
  // user would read them.
  const pending = () => element("status-pending").hidden ? 0 : parseInt(element("status-pending").textContent, 10);
  const listed = () => {
    if (pending() === 0) return [];
    key(" "); key("c");
    const items = element("picker-list").children.map(li => ({
      note: li.children[0].textContent.split("  ·  ")[1] ?? "", changed: /text changed or removed/.test(li.children[1].textContent),
    }));
    element("picker-input").dispatch("keydown", { key: "Escape" });
    return items;
  };
  const notes = () => listed().map(c => c.note);
  const remove = (index = 0) => {
    key(" "); key("c");
    for (let i = 0; i < index; i++) element("picker-input").dispatch("keydown", { key: "ArrowDown" });
    element("picker-input").dispatch("keydown", { key: "d", ctrlKey: true });
    element("picker-input").dispatch("keydown", { key: "Escape" });
  };
  const removeAll = () => { while (pending()) remove(); };
  return {
    element, document, window, messages, schemeListeners, env, receive, expectRequest, key, command, render,
    sendAll, bars, marked, pending, listed, notes, remove, removeAll, context: () => context, input: element("picker-input"),
  };
}

const scenarios = [];
const scenario = (name, run, options) => scenarios.push({ name, run, options });

scenario("Picker: scopes, scratchpad listing, filtering, caps, >30 results, opening, refresh, bookmarks", async page => {
  const { element, window, receive, expectRequest, key, render, listed, input } = page;
  receive({ type: "render", path: "/current.md", source: "", session: { session_id: "live" },
    documents: [{ path: "/current.md", label: "current.md", touched_at: 1 }] });
  await new Promise(setImmediate);
  window.dispatch("keydown", { key: "p", ctrlKey: true });
  assert.equal(element("picker-current").attributes["aria-pressed"], "true");
  assert.equal(element("picker-list").children[0].children[0].textContent, "current.md");
  input.dispatch("keydown", { key: "Tab" });
  expectRequest({ type: "list-scratchpad" });
  assert.equal(element("picker-scratchpad").attributes["aria-pressed"], "true");
  assert.match(element("picker-note").textContent, /Loading scratchpad/);
  input.value = "note-4";
  input.dispatch("input");
  const scratchFiles = Array.from({ length: 45 }, (_, index) => ({
    path: `/scratch/note-${index}.md`, label: `note-${index}.md`, touched_at: 100 - index,
  }));
  receive({ type: "scratchpad", available: true, documents: scratchFiles, found: 250, warnings: [] });
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
  receive({ type: "scratchpad", available: true, documents: scratchFiles, found: 45, warnings: [] }); // Late reply keeps scope.
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

  element("picker-bookmarks").dispatch("click");
  expectRequest({ type: "list-bookmarks" });
  assert.equal(element("picker-bookmarks").attributes["aria-pressed"], "true");
  assert.match(element("picker-note").textContent, /Loading bookmarks/);
  receive({ type: "bookmarks", documents: [], found: 0, warnings: [] });
  assert.match(element("picker-list").children[0].textContent, /No bookmarked Markdown files\. Add bookmarks = /);
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
});

scenario("Comment anchoring: ranges, document switches, moved and changed text, send lifecycle", async page => {
  const { element, messages, receive, key, command, render, sendAll, marked, pending, listed, remove } = page;
  await render();
  key("v"); key("j"); command("c Review both paragraphs");
  assert.deepEqual(marked(), [{ line: 0, endLine: 3, note: "Review both paragraphs" }]); // One bar for the range.
  command("c Another note");
  assert.deepEqual(marked().map(m => m.note), ["Review both paragraphs", "Another note"]);
  await render("/other.md");
  assert.equal(marked().length, 0);
  await render();
  assert.equal(marked().length, 2);
  await render("/comments.md", "New paragraph.\n\n" + source);
  assert.deepEqual(marked().map(m => m.line), [2, 4]); // Moved text keeps its comments.
  await render("/comments.md", source.replace("First paragraph.", "First, rewritten."));
  assert.deepEqual(marked().map(m => m.note), ["Another note"]); // Changed text is reported, never guessed at.
  assert.equal(listed()[0].changed, true);
  assert.equal(element("status-pending").textContent, "2 pending · 1 changed · S send");
  key("G"); key("]"); key("c"); // ]c stops on it too, and says what happened.
  assert.match(element("status-msg").textContent, /^comment 1 of 2 \(text changed or removed\): Review both paragraphs$/);
  sendAll();
  assert.match(messages.at(-1).text, /Review both paragraphs/);
  assert.match(messages.at(-1).text, /\(This text has since changed or been removed\.\)\n\nReview both paragraphs/);
  receive({ type: "send-result", purpose: "comments", ok: false, text: "failed" });
  await render();
  assert.equal(listed()[0].changed, false);
  sendAll();
  receive({ type: "send-result", purpose: "comments", ok: false, text: "failed" });
  assert.equal(marked().length, 2);
  sendAll();
  command("c Added while sending");
  receive({ type: "send-result", purpose: "comments", ok: true, text: "sent" });
  assert.deepEqual(marked().map(m => m.note), ["Added while sending"]); // Only the acknowledged batch disappears.
  remove();
  assert.equal(marked().length, 0);

  // With one of two identical blocks deleted, there is no telling which was
  // yours: the comment is reported, not moved to the other.
  const repeated = "Same.\n\nMiddle.\n\nSame.";
  await render("/repeated.md", repeated);
  key("G"); command("c On the second Same");
  await render("/repeated.md", "Same.\n\nMiddle.");
  assert.equal(marked().length, 0);
  assert.equal(listed()[0].changed, true);
  await render("/repeated.md", "Intro.\n\n" + repeated);
  assert.deepEqual(marked().map(m => m.line), [6]); // Both copies back: it follows the move.
  remove();
});

scenario("Composer: writing, editing, undo, the prompt's line references", async page => {
  const { element, messages, receive, key, render, sendAll, marked, pending, notes, remove, input } = page;
  await render();
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
  assert.equal(marked()[0].note, "First line\nsecond line");
  key("c"); composer.value = "Never mind"; composer.dispatch("keydown", { key: "Escape" });
  assert.equal(element("composer").hidden, true);
  assert.equal(pending(), 1); // Esc adds nothing.
  // C edits the comment on the cursor's block; Ctrl-E does it from the list.
  key("C");
  assert.equal(composer.value, "First line\nsecond line");
  composer.value = "Reworded";
  composer.dispatch("keydown", { key: "Enter" });
  assert.equal(marked()[0].note, "Reworded");
  assert.equal(pending(), 1);
  key("g"); key("g"); key("C");
  assert.match(element("status-msg").textContent, /no comment on this block · c adds one/);
  key(" "); key("c");
  input.dispatch("keydown", { key: "Enter" }); // The comments picker jumps; C then edits.
  key("C");
  assert.equal(composer.value, "Reworded");
  composer.dispatch("keydown", { key: "Escape" });
  // Nothing is lost by one wrong key: u takes back a removal or a discarded
  // note, and an empty note is no comment at all.
  key("u"); // The "Never mind" note discarded with Esc above comes back.
  assert.equal(element("composer").hidden, false);
  assert.equal(composer.value, "Never mind");
  composer.value = ""; composer.dispatch("keydown", { key: "Escape" }); // Nothing written: nothing kept.
  key("u");
  assert.match(element("status-msg").textContent, /nothing to undo/);
  key("g"); key("g"); key("c"); composer.value = ""; composer.dispatch("keydown", { key: "Enter" });
  assert.equal(pending(), 1); // Still only "Reworded".
  key("c"); composer.value = "Half a thought"; composer.dispatch("keydown", { key: "Escape" });
  assert.match(element("toast").textContent, /note discarded · u restores/);
  key("u");
  assert.equal(element("composer").hidden, false);
  assert.equal(composer.value, "Half a thought");
  composer.dispatch("keydown", { key: "Enter" });
  assert.equal(pending(), 2);
  key("g"); key("g"); key("C"); composer.value = ""; composer.dispatch("keydown", { key: "Enter" });
  assert.equal(pending(), 1); // Emptied: removed.
  assert.match(element("toast").textContent, /comment removed · u restores/);
  key("u");
  assert.equal(pending(), 2);
  key("g"); key("g"); key("x"); // x removes the comment on the cursor's block.
  assert.equal(pending(), 1);
  assert.match(element("toast").textContent, /comment removed · u restores/);
  key("u");
  assert.deepEqual(notes().sort(), ["Half a thought", "Reworded"]);
  remove(notes().indexOf("Half a thought"));

  // What the agent gets: where each quote is, as a line reference.
  sendAll();
  assert.match(messages.at(-1).text, /Comments on `\/comments\.md`:\n\n`\/comments\.md#L3`\n> Second paragraph\.\n\nReworded$/);
  receive({ type: "send-result", purpose: "comments", ok: true, text: "sent" });
  assert.equal(pending(), 0);
});

scenario("Review sheet: preview, opening line, one comment, no session", async page => {
  const { element, messages, receive, key, command, render, pending, remove, input } = page;
  await render();
  // Send all reviews first: the sheet shows the target and exactly what will
  // be pasted, with an opening line you can change.
  command("c Review this");
  const intro = element("review-intro");
  const beforeReview = messages.length;
  key("S");
  assert.equal(element("mode").textContent, "REVIEW");
  assert.equal(element("review").hidden, false);
  assert.equal(element("review-target").textContent, "Send 1 comment to / · Claude Code");
  assert.equal(intro.value, "Address these review comments."); // Until you send with another.
  intro.value = "Address these review comments.";
  intro.dispatch("input");
  assert.match(element("review-preview").textContent, /^Address these review comments\.\n\nComments on `\/comments\.md`:\n\n`\/comments\.md#L1`\n> First paragraph\.\n\nReview this$/);
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
  assert.equal(pending(), 0);
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
  remove();
});

scenario("Margin bars: one per comment, lanes, stepping with ]c and [c", async page => {
  const { element, document, receive, key, command, render, sendAll, bars, pending, notes } = page;
  await render();
  // One bar per comment beside its blocks: a range is one mark, not three, and
  // overlapping comments sit in lanes side by side.
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
});

scenario("Selection sends: comments inside go along, those reaching beyond stay", async page => {
  const { messages, receive, key, command, render, notes, remove } = page;
  await render();
  // Sending a selection takes the comments inside it along, and sends them.
  key("g"); key("g"); command("c On the first");
  key("G"); command("c On the last");
  key("g"); key("g"); key("v"); key("j"); key("s");
  const withComments = messages.at(-1);
  assert.equal(withComments.purpose, "comments");
  assert.match(withComments.text, /^`\/comments\.md#L1-3`\n> First paragraph\.\n>\n> Second paragraph\.\n\nComments on/);
  assert.match(withComments.text, /\n\n`\/comments\.md#L1`\n\nOn the first/); // Already quoted above: not again.
  assert.match(withComments.text, /On the first/);
  assert.doesNotMatch(withComments.text, /On the last/);
  receive({ type: "send-result", purpose: "comments", ok: true, text: "sent" });
  assert.deepEqual(notes(), ["On the last"]);
  remove();
  key("Escape");
  // A comment reaching beyond the selection stays behind.
  key("g"); key("g"); key("v"); key("j"); command("c On both");
  key("g"); key("g"); key("s");
  assert.equal(messages.at(-1).purpose, "selection");
  assert.doesNotMatch(messages.at(-1).text, /On both/);
  receive({ type: "send-result", purpose: "selection", ok: true, text: "sent" });
  remove();
});

scenario("Size limit: refused in the page, not left in flight", async page => {
  const { element, messages, command, render, sendAll, remove } = page;
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
  remove();
});

scenario("Leaving a document: the note is saved with it, a reload keeps it open", async page => {
  const { element, document, key, render, marked, pending, remove, input } = page;
  await render();
  // Leaving the document saves the note with it; the box does not follow.
  key("g"); key("g"); key("c");
  element("composer-input").value = "Keep the original target";
  await render("/other.md");
  assert.equal(element("composer").hidden, true);
  assert.match(element("toast").textContent, /note saved with its document/);
  assert.equal(marked().length, 0);
  await render();
  assert.equal(marked()[0].note, "Keep the original target");
  key("g"); key("g"); key("c"); // An empty note is just closed.
  await render("/other.md");
  assert.equal(element("composer").hidden, true);
  await render();
  assert.equal(pending(), 1);
  // A reload while writing a note on a range keeps the note open and on its
  // text, and no question interrupts it.
  remove();
  key("g"); key("g"); key("v"); key("j"); key("c");
  element("composer-input").value = "Written during a reload";
  await render("/comments.md", "Inserted.\n\n" + source);
  assert.equal(element("mode").textContent, "COMMENT");
  assert.equal(element("composer").hidden, false);
  element("composer-input").dispatch("keydown", { key: "Enter" });
  assert.deepEqual(marked(), [{ line: 2, endLine: 5, note: "Written during a reload" }]); // Followed its text.
});

scenario("Real Markdown: frontmatter, headings, list items, fences and quotes each map to their own lines", async page => {
  const { messages, receive, key, command, render, sendAll, marked } = page;
  const plan = [
    "---", "title: Plan", "---", "# Phase 1", "",
    "- First item", "  continued", "- Second item", "  - nested", "",
    "```js", "const x = 1;", "```", "", "> Quoted", "", "Closing paragraph.",
  ].join("\n");
  await render("/plan.md", plan);
  for (let i = 0; i < 7; i++) { command(`c Note ${i}`); key("j"); }
  assert.deepEqual(marked().map(m => [m.line, m.endLine]), [[0, 3], [3, 4], [5, 7], [7, 10], [10, 13], [14, 15], [16, 17]]);
  sendAll();
  const refs = [...messages.at(-1).text.matchAll(/`\/plan\.md#(L[\d-]+)`/g)].map(m => m[1]);
  // The last list item ends at its text, not at the blank line markdown-it gives it.
  assert.deepEqual(refs, ["L1-3", "L4", "L6-7", "L8-9", "L11-13", "L15", "L17"]);
  assert.match(messages.at(-1).text, /`\/plan\.md#L6-7`\n> - First item\n>   continued\n\nNote 2/);
  receive({ type: "send-result", purpose: "comments", ok: true, outcome: "pasted", text: "sent" });
}, { markdown: true });

scenario("Keyboard after a stray click: the review sheet keeps the keys, Esc still closes it", async page => {
  const { element, key, command, render, input } = page;
  await render();
  command("c Review this");
  key("S");
  key("j"); // Focus was lost to a click; the key brings it back instead of being dropped.
  assert.equal(element("mode").textContent, "REVIEW");
  key("Escape");
  assert.equal(element("review").hidden, true);
  assert.equal(element("mode").textContent, "NORMAL");
  key("c"); element("composer-input").value = "Half written";
  key("Escape"); // Same for the note box.
  assert.equal(element("composer").hidden, true);
  assert.match(element("toast").textContent, /note discarded · u restores/);
});

scenario("Sends in flight: an edit made meanwhile stays pending, a removal is not undone into a resend", async page => {
  const { element, receive, key, command, render, sendAll, notes, remove, input } = page;
  await render();
  command("c First version");
  sendAll();
  key("C"); element("composer-input").value = "First version, and more";
  element("composer-input").dispatch("keydown", { key: "Enter" });
  receive({ type: "send-result", purpose: "comments", ok: true, outcome: "copied", text: "copied" });
  key("y"); // What was pasted is sent; what was added after it was not.
  assert.deepEqual(notes(), ["First version, and more"]);
  remove();
  command("c Removed while sending");
  sendAll();
  remove();
  receive({ type: "send-result", purpose: "comments", ok: true, outcome: "pasted", text: "sent" });
  key("u");
  assert.match(element("status-msg").textContent, /nothing to undo/);
  assert.deepEqual(notes(), []);
});

scenario("Undo after a send: a discarded edit comes back as a new comment, on its own document", async page => {
  const { element, receive, key, command, render, sendAll, pending, notes, remove, input } = page;
  const composer = element("composer-input");
  await render();
  command("c Keep me");
  key("C"); composer.value = "Keep me, reworded";
  composer.dispatch("keydown", { key: "Escape" });
  sendAll();
  receive({ type: "send-result", purpose: "comments", ok: true, outcome: "pasted", text: "sent" });
  assert.equal(pending(), 0);
  key("u");
  assert.equal(composer.value, "Keep me, reworded");
  composer.dispatch("keydown", { key: "Enter" });
  assert.deepEqual(notes(), ["Keep me, reworded"]);
  remove();
  key("j"); key("c"); composer.value = "About the second paragraph";
  composer.dispatch("keydown", { key: "Escape" });
  await render("/other.md");
  key("u");
  assert.match(element("status-msg").textContent, /that note is on \/comments\.md · u there restores it/);
  assert.equal(element("composer").hidden, true);
  await render();
  key("u");
  assert.equal(composer.value, "About the second paragraph");
  assert.equal(element("composer-quote").textContent, "Second paragraph.");
});

scenario("Repeated text: a comment still on its lines keeps them when a copy appears", async page => {
  const { element, messages, receive, command, render, sendAll, marked, pending } = page;
  await render("/plan.md", "### Tests\n\nOne.");
  command("c Cover the edge cases");
  await render("/plan.md", "### Tests\n\nOne.\n\n### Tests\n\nTwo."); // Another phase with its own Tests.
  assert.deepEqual(marked().map(m => m.line), [0]);
  assert.equal(element("status-pending").textContent, "1 pending · S send");
  await render("/plan.md", "Intro.\n\n### Tests\n\nOne.\n\n### Tests\n\nTwo."); // Then it moves with the first copy.
  assert.deepEqual(marked().map(m => m.line), [2]);
  sendAll();
  assert.match(messages.at(-1).text, /`\/plan\.md#L3`\n> ### Tests\n\nCover the edge cases$/);
  receive({ type: "send-result", purpose: "comments", ok: true, outcome: "pasted", text: "sent" });
});

scenario("Another session on the same file: the open note is saved with its own session", async page => {
  const { element, document, receive, key, render, pending, notes, input } = page;
  const show = async session_id => {
    receive({ type: "render", path: "/repo/PLAN.md", source, session: { session_id }, documents: [] });
    await new Promise(setImmediate);
  };
  await show("a");
  key("c"); element("composer-input").value = "From A";
  await show("b");
  assert.equal(element("composer").hidden, true);
  assert.match(element("toast").textContent, /note saved with its document/);
  assert.equal(pending(), 0);
  await show("a");
  assert.deepEqual(notes(), ["From A"]);
});

scenario("Comments elsewhere: jumping reaches the session's documents, and says so for others", async page => {
  const { element, messages, receive, key, command, render, notes, input } = page;
  const show = async (path, documents) => {
    receive({ type: "render", path, source, session: { session_id: "s" }, documents });
    await new Promise(setImmediate);
  };
  const sessionFiles = [{ path: "/plan.md", label: "plan.md", touched_at: 2 }, { path: "/notes.md", label: "notes.md", touched_at: 1 }];
  await show("/bookmark.md", sessionFiles); // Opened from Bookmarks: not one of the session's documents.
  command("c On the bookmark");
  await show("/plan.md", sessionFiles);
  command("c On the plan");
  await show("/notes.md", sessionFiles);
  const before = messages.length;
  key(" "); key("c");
  input.value = "bookmark"; input.dispatch("input");
  input.dispatch("keydown", { key: "Enter" });
  assert.equal(messages.length, before);
  assert.match(element("status-msg").textContent, /\/bookmark\.md is not in this session's documents · Ctrl-P opens it/);
  key(" "); key("c");
  input.value = "plan"; input.dispatch("input");
  input.dispatch("keydown", { key: "Enter" });
  assert.equal(messages.at(-1).type, "switch");
  assert.equal(messages.at(-1).path, "/plan.md");
});

scenario("Links: HTML and SVG anchors never navigate the webview", async page => {
  const { document, messages } = page;
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
});

scenario("Empty session: placeholder, inert actions, picker hint, first document, scan notes", async page => {
  const { element, window, messages, receive, key, render, input } = page;
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
});

scenario("Session isolation: drafts, delivery identity, acknowledgments, copied drafts, stale listings", async page => {
  const { element, window, messages, receive, key, command, render, sendAll, pending, notes, remove, context, input } = page;
  const project = async (id, text, file = "README.md") => {
    receive({ type: "render", path: `/${id}/${file}`, file_identity: `/${id}/${file}`, source: text,
      session: { session_id: id, cwd: `/${id}` }, documents: [] });
    await new Promise(setImmediate);
  };
  await project("a", "Project A"); command("c Fix A");
  const aContext = context();
  // A resume is the same session: its comments stay.
  receive({ type: "render", path: "/a/README.md", file_identity: "/a/README.md", source: "Project A",
    session: { session_id: "a", cwd: "/a" }, documents: [],
    context: { generation: 99, session: { ...aContext.session, incarnation: "resumed" } } });
  await new Promise(setImmediate);
  assert.equal(pending(), 1);
  // Another session parks them, without asking; coming back restores them.
  await project("b", "Project B");
  assert.equal(pending(), 0);
  assert.doesNotMatch(element("status-doc").textContent, /pending/);
  await project("a", "Project A");
  assert.equal(pending(), 1);
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
  assert.equal(pending(), 1);
  remove();
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
  assert.equal(pending(), 1);
  sendAll();
  receive({ type: "send-result", purpose: "comments", ok: true, outcome: "copied", text: "copied" });
  key("y");
  assert.equal(pending(), 0);
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
  assert.deepEqual(notes(), ["Written while asked"]);
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
  assert.equal(pending(), 0);
  await project("b", "Project B"); command("c Fix B"); sendAll();
  const bSend = messages.at(-1);
  assert.equal(bSend.type, "send");
  assert.equal(bSend.context.session.key.session_id, "b");
  assert.match(bSend.text, /Fix B/);
  assert.doesNotMatch(bSend.text, /Fix A/);
  assert.match(bSend.text, /(^|[^/])README\.md/, "paths are relative to the session's cwd");
  assert.doesNotMatch(bSend.text, /\/b\/README\.md/);
  assert.match(element("status-doc").textContent, /^b · Claude Code › /);
  assert.equal(pending(), 1);
  receive({ type: "send-result", purpose: "comments", ok: true, request_id: bSend.request_id - 1, text: "stale" });
  assert.equal(pending(), 1);
  receive({ type: "send-result", purpose: "comments", ok: true, outcome: "copied", text: "copied" });
  assert.equal(pending(), 1); // Clipboard fallback keeps the draft until answered.
  key("y");
  assert.equal(pending(), 0); // So it cannot go out twice.
  await project("b", "Project B"); command("c Fix B again"); sendAll();
  receive({ type: "send-result", purpose: "comments", ok: true, outcome: "pasted", text: "sent" });
  assert.equal(pending(), 0);
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
});

scenario("Themes: auto follows the system, pinned themes stay, font overrides, diagram colors", async page => {
  const { element, document, schemeListeners, env, receive, input } = page;
  // Auto follows the system between the light and dark themes, and diagrams
  // follow along; a pinned theme ignores the system.
  const root = document.documentElement;
  receive({ type: "theme", theme: { name: "auto", font_family: "My Mono", font_size: 14 } });
  assert.equal(root.dataset.theme, "light");
  assert.equal(root.style.get("--mono"), '"My Mono", ui-monospace, Menlo, monospace');
  assert.equal(root.style.get("--mono-size"), "14px");
  const initsBefore = env.mermaidInits;
  env.systemDark = true;
  for (const fn of schemeListeners) fn({ matches: true });
  assert.equal(root.dataset.theme, "dark");
  assert.equal(env.mermaidInits, initsBefore + 1);
  receive({ type: "theme", theme: { name: "light", font_family: null, font_size: null } });
  assert.equal(root.dataset.theme, "light");
  assert.equal(root.style.get("--mono"), undefined);
  const pinnedInits = env.mermaidInits;
  for (const fn of schemeListeners) fn({ matches: true });
  assert.equal(root.dataset.theme, "light");
  assert.equal(env.mermaidInits, pinnedInits);
  env.systemDark = false;
  // Diagrams get plain colors: Mermaid cannot parse CSS color-mix().
  env.themeTokens = { bg: "#ffffff", fg: "#000000", accent: "#0000ff", annotate: "#ff0000", muted: "#555555", "code-bg": "#eeeeee" };
  receive({ type: "theme", theme: { name: "light", font_family: null, font_size: null } });
  assert.equal(env.mermaidOptions.theme, "base");
  assert.equal(env.mermaidOptions.themeVariables.primaryColor, "#d6d6ff");
  assert.equal(env.mermaidOptions.themeVariables.secondaryColor, "#ffd6d6");
  for (const value of Object.values(env.mermaidOptions.themeVariables)) assert.doesNotMatch(String(value), /color-mix/);
});

const adopt = (parent, children) => children.map(child => Object.assign(child, { parentElement: parent }));

// Just the selectors app.js uses: tags, classes and attributes, joined by
// the child combinator, with :scope for the element searched from.
function matchesSelector(el, selector, scope) {
  return selector.split(",").some(alternative => {
    const parts = alternative.trim().split(/\s*>\s*/);
    let node = el;
    for (let i = parts.length - 1; i >= 0; i--, node = node?.parentElement) {
      if (!node || !(parts[i] === ":scope" ? node === scope : matchesSimple(node, parts[i]))) return false;
    }
    return true;
  });
}

function matchesSimple(el, simple) {
  const [, tag, classes, attributes] = simple.match(/^([a-z0-9]*)((?:\.[\w-]+)*)((?:\[[\w-]+\])*)$/)
    ?? assert.fail(`the test DOM does not support the selector ${simple}`);
  const dataKey = name => name.slice(5).replace(/-(\w)/g, (_, c) => c.toUpperCase());
  return (!tag || el.tagName === tag.toUpperCase())
    && classes.split(".").filter(Boolean).every(c => el.className.split(" ").includes(c))
    && [...attributes.matchAll(/\[([\w-]+)\]/g)].every(([, name]) =>
      (name.startsWith("data-") ? el.dataset[dataKey(name)] : el.attributes[name]) !== undefined);
}

// Enough of an HTML parser for markdown-it's output, which is well formed.
function parseHtml(html) {
  const root = new Element();
  const stack = [root];
  const voids = new Set(["br", "hr", "img", "input"]);
  for (const [, closing, tag, attributes, text] of html.matchAll(/<(\/?)([a-z0-9]+)([^>]*)>|([^<]+)/g)) {
    const top = stack.at(-1);
    if (text !== undefined) {
      for (const node of stack) node.textContent = (node.textContent ?? "") + text;
    } else if (closing) {
      stack.pop();
    } else {
      const el = new Element();
      el.tagName = tag.toUpperCase();
      for (const [, name, value = ""] of attributes.matchAll(/([\w-]+)(?:="([^"]*)")?/g)) {
        if (name === "class") el.className = value;
        else if (name.startsWith("data-")) el.dataset[name.slice(5).replace(/-(\w)/g, (_, c) => c.toUpperCase())] = value;
        else el.attributes[name] = value;
      }
      top.append(el);
      if (!voids.has(tag)) stack.push(el);
    }
  }
  return root.children;
}

async function main() {
  for (const { name, run, options } of scenarios) {
    await run(boot(options));
    console.log(`Passed ${name}.`);
  }
}

main().catch(error => { console.error(error); process.exitCode = 1; });
