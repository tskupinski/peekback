(() => {
  const $ = (id) => document.getElementById(id);
  const docEl = $("doc");
  const bannerEl = $("banner");
  const sessionsEl = $("sessions");
  const documentsEl = $("documents");
  const toastEl = $("toast");
  const toolbarEl = $("toolbar");
  const helpEl = $("help");
  const modeEl = $("mode");
  const statusDocEl = $("status-doc");
  const statusMsgEl = $("status-msg");
  const cmdlineEl = $("cmdline");
  const cmdPrefixEl = $("cmd-prefix");
  const cmdInput = $("cmd-input");
  const completionsEl = $("completions");

  const darkMode = () => window.matchMedia("(prefers-color-scheme: dark)").matches;

  // ---------------------------------------------------------------- rendering

  const md = window
    .markdownit({
      html: true,
      linkify: true,
      highlight(code, lang) {
        if (lang === "mermaid") return "";
        if (lang && hljs.getLanguage(lang)) {
          return hljs.highlight(code, { language: lang, ignoreIllegals: true }).value;
        }
        return "";
      },
    })
    .use(window.markdownitFootnote)
    .use(window.markdownitTaskLists, { label: true })
    .use(sourceLines);

  function sourceLines(md) {
    md.core.ruler.push("source_lines", (state) => {
      for (const token of state.tokens) {
        if (token.block && token.map && token.nesting !== -1) {
          token.attrSet("data-source-line", String(token.map[0]));
          token.attrSet("data-source-end", String(token.map[1]));
        }
      }
    });
  }

  mermaid.initialize({ startOnLoad: false, theme: darkMode() ? "dark" : "default" });

  const state = {
    doc: null, // { path, source, label, sessionId, documents }
    sessions: { sessions: [], current: null },
    blocks: [],
    cursor: 0,
    mode: "normal", // normal | visual | search | command
    anchor: null, // visual mode anchor block index
    pendingKey: null,
    search: { query: "", matches: [], index: -1 },
  };

  async function render(message) {
    const scrollY = window.scrollY;
    const previousLine = state.blocks[state.cursor]?.dataset.sourceLine;
    state.doc = {
      path: message.path,
      source: message.source,
      sessionId: message.session?.session_id ?? null,
      documents: message.documents,
      label: labelFor(message.path, message.documents),
    };
    document.title = `${state.doc.label} - peekback`;
    bannerEl.hidden = true;

    docEl.innerHTML = md.render(message.source);
    await renderMermaid();
    renderMathInElement(docEl, {
      delimiters: [
        { left: "$$", right: "$$", display: true },
        { left: "$", right: "$", display: false },
      ],
      throwOnError: false,
    });
    window.scrollTo(0, scrollY);

    collectBlocks();
    state.cursor = previousLine === undefined ? 0 : nearestBlock(Number(previousLine));
    if (state.mode === "visual") setMode("normal");
    if (state.search.query) runSearch(state.search.query);
    paintCursor(false);
    renderDocuments();
    renderStatus();
  }

  function labelFor(path, documents) {
    return documents.find((d) => d.path === path)?.label ?? path.split("/").pop();
  }

  async function renderMermaid() {
    const nodes = [];
    for (const code of docEl.querySelectorAll("pre > code.language-mermaid")) {
      const pre = code.parentElement;
      const container = document.createElement("pre");
      container.className = "mermaid";
      container.textContent = code.textContent;
      container.dataset.sourceLine = pre.dataset.sourceLine ?? code.dataset.sourceLine;
      container.dataset.sourceEnd = pre.dataset.sourceEnd ?? code.dataset.sourceEnd;
      pre.replaceWith(container);
      nodes.push(container);
    }
    if (nodes.length === 0) return;
    try {
      await mermaid.run({ nodes });
    } catch (e) {
      console.error("mermaid", e);
    }
  }

  window.matchMedia("(prefers-color-scheme: dark)").addEventListener("change", () => {
    mermaid.initialize({ startOnLoad: false, theme: darkMode() ? "dark" : "default" });
    if (state.doc) render({ ...state.doc, session: { session_id: state.doc.sessionId } });
  });

  // ------------------------------------------------------------------ blocks

  // The cursor moves over top-level blocks, with list items as their own
  // stops so a single bullet can be picked out.
  function collectBlocks() {
    const blocks = [];
    for (const child of docEl.children) {
      if (child.matches("ul, ol")) {
        for (const li of child.querySelectorAll(":scope > li")) {
          if (li.dataset.sourceLine !== undefined) blocks.push(li);
        }
      } else if (child.dataset.sourceLine !== undefined) {
        blocks.push(child);
      }
    }
    state.blocks = blocks;
    if (state.cursor >= blocks.length) state.cursor = Math.max(0, blocks.length - 1);
  }

  function nearestBlock(line) {
    let best = 0;
    for (let i = 0; i < state.blocks.length; i++) {
      if (Number(state.blocks[i].dataset.sourceLine) <= line) best = i;
      else break;
    }
    return best;
  }

  function selectionRange() {
    if (state.mode === "visual" && state.anchor !== null) {
      return [Math.min(state.anchor, state.cursor), Math.max(state.anchor, state.cursor)];
    }
    return [state.cursor, state.cursor];
  }

  function paintCursor(scroll = true) {
    const [from, to] = selectionRange();
    state.blocks.forEach((block, i) => {
      block.classList.toggle("cursor", i === state.cursor);
      block.classList.toggle("selected", state.mode === "visual" && i >= from && i <= to);
    });
    const current = state.blocks[state.cursor];
    if (scroll && current) current.scrollIntoView({ block: "nearest" });
  }

  function sourceOf(from, to) {
    const start = Number(state.blocks[from].dataset.sourceLine);
    const end = Number(state.blocks[to].dataset.sourceEnd);
    return state.doc.source.split("\n").slice(start, end).join("\n").trimEnd();
  }

  function moveCursor(index) {
    if (state.blocks.length === 0) return;
    state.cursor = Math.max(0, Math.min(state.blocks.length - 1, index));
    paintCursor();
  }

  function topVisibleBlock() {
    const top = window.scrollY + 8;
    let index = state.blocks.findIndex((b) => b.offsetTop + b.offsetHeight > top);
    return index < 0 ? state.blocks.length - 1 : index;
  }

  function halfPage(direction) {
    window.scrollBy({ top: (direction * window.innerHeight) / 2 });
    requestAnimationFrame(() => {
      state.cursor = topVisibleBlock();
      paintCursor(false);
    });
  }

  function nextHeading(direction) {
    const isHeading = (b) => /^H[1-6]$/.test(b.tagName);
    let i = state.cursor + direction;
    while (i >= 0 && i < state.blocks.length) {
      if (isHeading(state.blocks[i])) return moveCursor(i);
      i += direction;
    }
  }

  // ------------------------------------------------------------------ modes

  function setMode(mode) {
    state.mode = mode;
    if (mode !== "visual") state.anchor = null;
    modeEl.textContent = mode.toUpperCase();
    modeEl.dataset.mode = mode;
    paintCursor(false);
  }

  function setMessage(text) {
    statusMsgEl.textContent = text;
  }

  function renderStatus() {
    statusDocEl.textContent = state.doc ? state.doc.label : "";
  }

  // -------------------------------------------------------------- actions

  function blockquote(text) {
    return text
      .split("\n")
      .map((line) => (line ? `> ${line}` : ">"))
      .join("\n");
  }

  function actOnSelection(action) {
    if (!state.doc || state.blocks.length === 0) return;
    const [from, to] = selectionRange();
    const text = sourceOf(from, to);
    perform(action, text);
    if (state.mode === "visual") setMode("normal");
  }

  function perform(action, text) {
    if (!text) return;
    if (action === "copy") send({ type: "copy", text });
    if (action === "send") send({ type: "send", text: blockquote(text) });
  }

  // -------------------------------------------------------------- search

  const highlightsSupported = "highlights" in CSS && typeof Highlight === "function";

  function runSearch(query) {
    state.search.query = query;
    state.search.matches = [];
    state.search.index = -1;
    if (highlightsSupported) CSS.highlights.delete("search");
    state.blocks.forEach((b) => b.classList.remove("match"));
    if (!query) return;

    const needle = query.toLowerCase();
    const ranges = [];
    const walker = document.createTreeWalker(docEl, NodeFilter.SHOW_TEXT);
    for (let node = walker.nextNode(); node; node = walker.nextNode()) {
      const text = node.textContent.toLowerCase();
      let at = text.indexOf(needle);
      while (at >= 0) {
        const range = new Range();
        range.setStart(node, at);
        range.setEnd(node, at + needle.length);
        ranges.push(range);
        at = text.indexOf(needle, at + needle.length);
      }
    }
    if (highlightsSupported) CSS.highlights.set("search", new Highlight(...ranges));
    state.search.matches = ranges.map((r) => {
      const block = r.startContainer.parentElement.closest("[data-source-line]");
      const index = state.blocks.findIndex((b) => b === block || b.contains(block));
      if (!highlightsSupported && index >= 0) state.blocks[index].classList.add("match");
      return { range: r, block: index };
    });
    setMessage(ranges.length ? `${ranges.length} match${ranges.length === 1 ? "" : "es"}` : "no matches");
  }

  function gotoMatch(step) {
    const { matches } = state.search;
    if (matches.length === 0) return;
    state.search.index = (state.search.index + step + matches.length) % matches.length;
    const match = matches[state.search.index];
    if (match.block >= 0) moveCursor(match.block);
    const rect = match.range.getBoundingClientRect();
    if (rect.top < 0 || rect.bottom > window.innerHeight) {
      window.scrollBy({ top: rect.top - window.innerHeight / 3 });
    }
    setMessage(`match ${state.search.index + 1} of ${matches.length}`);
  }

  function clearSearch() {
    runSearch("");
    setMessage("");
  }

  // ------------------------------------------------------------- commands

  const commands = {
    q: () => send({ type: "hide" }),
    quit: () => send({ type: "hide" }),
    doc: (arg) => switchTo(documentCandidates(), arg, "document"),
    session: (arg) => switchTo(sessionCandidates(), arg, "session"),
    send: () => actOnSelection("send"),
    copy: () => actOnSelection("copy"),
    help: () => toggleHelp(),
  };

  function documentCandidates() {
    if (!state.doc) return [];
    return state.doc.documents.map((d) => ({
      label: d.label,
      run: () => send({ type: "switch", session_id: state.doc.sessionId, path: d.path }),
    }));
  }

  function sessionCandidates() {
    return state.sessions.sessions.map((s) => ({
      label: sessionName(s),
      run: () => send({ type: "switch", session_id: s.session_id }),
    }));
  }

  function switchTo(candidates, arg, what) {
    const match = fuzzy(candidates, arg)[0];
    if (!match) return setMessage(`no ${what} matching ${arg}`);
    match.run();
  }

  function fuzzy(candidates, query) {
    const q = (query || "").toLowerCase();
    if (!q) return candidates;
    const scored = candidates
      .map((c) => ({ c, score: fuzzyScore(c.label.toLowerCase(), q) }))
      .filter((x) => x.score >= 0)
      .sort((a, b) => b.score - a.score);
    return scored.map((x) => x.c);
  }

  function fuzzyScore(text, query) {
    if (text.includes(query)) return 1000 - text.indexOf(query);
    let ti = 0;
    let score = 0;
    for (const ch of query) {
      const at = text.indexOf(ch, ti);
      if (at < 0) return -1;
      score += at === ti ? 2 : 1;
      ti = at + 1;
    }
    return score;
  }

  function runCommand(line) {
    const [name, ...rest] = line.trim().split(/\s+/);
    if (!name) return;
    const handler = commands[name];
    if (!handler) return setMessage(`unknown command :${name}`);
    handler(rest.join(" "));
  }

  // The command line doubles as the search field; `prefix` tells them apart.
  let cmdPrefix = ":";
  let completionList = [];
  let completionIndex = -1;

  function openCmdline(prefix) {
    cmdPrefix = prefix;
    cmdPrefixEl.textContent = prefix;
    cmdInput.value = "";
    cmdlineEl.hidden = false;
    hideCompletions();
    setMode(prefix === ":" ? "command" : "search");
    cmdInput.focus();
  }

  function closeCmdline() {
    cmdlineEl.hidden = true;
    hideCompletions();
    cmdInput.blur();
    setMode("normal");
  }

  function completionsFor(value) {
    const [name, ...rest] = value.split(/\s+/);
    const arg = rest.join(" ");
    if (rest.length === 0) {
      return Object.keys(commands)
        .filter((c) => c.startsWith(name))
        .map((c) => ({ label: c, apply: () => `${c} ` }));
    }
    const source = name === "doc" ? documentCandidates() : name === "session" ? sessionCandidates() : [];
    return fuzzy(source, arg).map((c) => ({ label: c.label, apply: () => `${name} ${c.label}` }));
  }

  function showCompletions() {
    completionList = completionsFor(cmdInput.value);
    completionIndex = -1;
    completionsEl.replaceChildren(
      ...completionList.slice(0, 12).map((c) => {
        const li = document.createElement("li");
        li.textContent = c.label;
        return li;
      })
    );
    completionsEl.hidden = completionList.length === 0;
  }

  function hideCompletions() {
    completionsEl.hidden = true;
    completionList = [];
    completionIndex = -1;
  }

  function cycleCompletion(step) {
    if (completionList.length === 0) showCompletions();
    if (completionList.length === 0) return;
    completionIndex = (completionIndex + step + completionList.length) % completionList.length;
    cmdInput.value = completionList[completionIndex].apply();
    [...completionsEl.children].forEach((li, i) => li.classList.toggle("active", i === completionIndex));
  }

  cmdInput.addEventListener("input", () => {
    if (cmdPrefix === "/") runSearch(cmdInput.value);
    else showCompletions();
  });

  cmdInput.addEventListener("keydown", (event) => {
    event.stopPropagation();
    if (event.key === "Escape") {
      if (cmdPrefix === "/") clearSearch();
      closeCmdline();
    } else if (event.key === "Enter") {
      const value = cmdInput.value;
      closeCmdline();
      if (cmdPrefix === "/") gotoMatch(1);
      else runCommand(value);
    } else if (event.key === "Tab") {
      event.preventDefault();
      if (cmdPrefix === ":") cycleCompletion(event.shiftKey ? -1 : 1);
    }
  });

  function toggleHelp() {
    helpEl.hidden = !helpEl.hidden;
  }

  // ---------------------------------------------------------------- keys

  const pendingTimeout = 800;
  let pendingTimer = null;

  function setPending(key) {
    state.pendingKey = key;
    clearTimeout(pendingTimer);
    pendingTimer = setTimeout(() => (state.pendingKey = null), pendingTimeout);
    setMessage(key);
  }

  function takePending() {
    const key = state.pendingKey;
    state.pendingKey = null;
    clearTimeout(pendingTimer);
    setMessage("");
    return key;
  }

  window.addEventListener("keydown", (event) => {
    if (event.metaKey || event.ctrlKey || event.altKey) return;
    if (state.mode === "command" || state.mode === "search") return;
    if (!helpEl.hidden && event.key !== "?") {
      helpEl.hidden = true;
      if (event.key === "Escape") return;
    }
    const key = event.key;
    const pending = takePending();
    let handled = true;

    if (pending === "g") {
      if (key === "g") moveCursor(0);
      else handled = false;
    } else if (pending === "]" || pending === "[") {
      const direction = pending === "]" ? 1 : -1;
      if (key === pending) nextHeading(direction);
      else if (key === "d") cycleDocument(direction);
      else if (key === "s") cycleSession(direction);
      else handled = false;
    } else {
      switch (key) {
        case "j": moveCursor(state.cursor + 1); break;
        case "k": moveCursor(state.cursor - 1); break;
        case "d": halfPage(1); break;
        case "u": halfPage(-1); break;
        case "G": moveCursor(state.blocks.length - 1); break;
        case "g": case "]": case "[": setPending(key); break;
        case "v":
          if (state.mode === "visual") setMode("normal");
          else { state.anchor = state.cursor; setMode("visual"); }
          break;
        case "y": actOnSelection("copy"); break;
        case "s": actOnSelection("send"); break;
        case "/": openCmdline("/"); break;
        case ":": openCmdline(":"); break;
        case "n": gotoMatch(1); break;
        case "N": gotoMatch(-1); break;
        case "?": toggleHelp(); break;
        case "Escape":
          if (state.mode === "visual") setMode("normal");
          else if (state.search.query) clearSearch();
          else send({ type: "hide" });
          break;
        default: handled = false;
      }
    }
    if (handled) event.preventDefault();
  });

  function cycleDocument(direction) {
    if (!state.doc) return;
    const docs = state.doc.documents;
    const i = docs.findIndex((d) => d.path === state.doc.path);
    const next = docs[(i + direction + docs.length) % docs.length];
    if (next) send({ type: "switch", session_id: state.doc.sessionId, path: next.path });
  }

  function cycleSession(direction) {
    const list = state.sessions.sessions;
    if (list.length === 0) return;
    const i = list.findIndex((s) => s.session_id === state.sessions.current);
    const next = list[(i + direction + list.length) % list.length];
    send({ type: "switch", session_id: next.session_id });
  }

  // ------------------------------------------------------ mouse selection

  function mouseSelectionText() {
    const selection = window.getSelection();
    if (!selection || selection.isCollapsed || selection.rangeCount === 0) return null;
    const range = selection.getRangeAt(0);
    if (!docEl.contains(range.commonAncestorContainer)) return null;
    return { text: selection.toString().trim(), rect: range.getBoundingClientRect() };
  }

  function placeToolbar() {
    const found = mouseSelectionText();
    if (!found || !found.text) {
      toolbarEl.hidden = true;
      return;
    }
    toolbarEl.hidden = false;
    const width = toolbarEl.offsetWidth;
    const left = Math.max(8, Math.min(window.innerWidth - width - 8, found.rect.left + found.rect.width / 2 - width / 2));
    const top = found.rect.top - toolbarEl.offsetHeight - 8;
    toolbarEl.style.left = `${left}px`;
    toolbarEl.style.top = `${top < 8 ? found.rect.bottom + 8 : top}px`;
  }

  document.addEventListener("mouseup", () => setTimeout(placeToolbar, 0));
  document.addEventListener("selectionchange", () => {
    if (window.getSelection()?.isCollapsed) toolbarEl.hidden = true;
  });

  toolbarEl.addEventListener("mousedown", (event) => event.preventDefault());
  toolbarEl.addEventListener("click", (event) => {
    const button = event.target.closest("button");
    if (!button) return;
    const found = mouseSelectionText();
    if (found) perform(button.dataset.action, found.text);
    window.getSelection()?.removeAllRanges();
    toolbarEl.hidden = true;
  });

  docEl.addEventListener("click", (event) => {
    const block = event.target.closest("[data-source-line]");
    const index = state.blocks.findIndex((b) => b === block || b.contains(block));
    if (index >= 0 && window.getSelection()?.isCollapsed) {
      state.cursor = index;
      paintCursor(false);
    }
  });

  // -------------------------------------------------------------- sidebar

  function sessionName(s) {
    return s.cwd.split("/").pop() || s.cwd;
  }

  function ago(unixSeconds) {
    const s = Math.max(0, Math.floor(Date.now() / 1000) - unixSeconds);
    if (s < 60) return "just now";
    if (s < 3600) return `${Math.floor(s / 60)}m ago`;
    if (s < 86400) return `${Math.floor(s / 3600)}h ago`;
    return `${Math.floor(s / 86400)}d ago`;
  }

  function item({ name, meta, title, current, onClick }) {
    const li = document.createElement("li");
    li.className = "item" + (current ? " current" : "");
    li.title = title;
    const nameEl = document.createElement("span");
    nameEl.className = "name";
    nameEl.textContent = name;
    const metaEl = document.createElement("span");
    metaEl.className = "meta";
    metaEl.textContent = meta;
    li.append(nameEl, metaEl);
    li.addEventListener("click", onClick);
    return li;
  }

  function renderSessions() {
    const { sessions, current } = state.sessions;
    sessionsEl.replaceChildren(
      ...sessions.map((s) =>
        item({
          name: sessionName(s),
          meta: ago(s.last_active_at),
          title: `${s.cwd}\n${s.session_id}`,
          current: s.session_id === current,
          onClick: () => send({ type: "switch", session_id: s.session_id }),
        })
      )
    );
    if (sessions.length === 0) {
      const li = document.createElement("li");
      li.className = "empty";
      li.textContent = "No live sessions";
      sessionsEl.append(li);
    }
  }

  function renderDocuments() {
    if (!state.doc) return;
    documentsEl.replaceChildren(
      ...state.doc.documents.map((d) => {
        const slash = d.label.lastIndexOf("/");
        return item({
          name: slash >= 0 ? d.label.slice(slash + 1) : d.label,
          meta: slash >= 0 ? d.label.slice(0, slash) : "",
          title: d.path,
          current: d.path === state.doc.path,
          onClick: () => send({ type: "switch", session_id: state.doc.sessionId, path: d.path }),
        });
      })
    );
  }

  setInterval(renderSessions, 30000);

  // ---------------------------------------------------------------- misc

  let toastTimer = null;
  function toast(text) {
    toastEl.textContent = text;
    toastEl.hidden = false;
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => (toastEl.hidden = true), 4000);
    setMessage(text);
  }

  document.addEventListener("click", (event) => {
    const link = event.target.closest("a[href]");
    if (!link) return;
    const href = link.getAttribute("href");
    if (href.startsWith("#")) return;
    event.preventDefault();
    if (/^https?:\/\//.test(href)) send({ type: "open-external", url: href });
  });

  function send(message) {
    window.ipc.postMessage(JSON.stringify(message));
  }

  window.peekback = {
    receive(message) {
      switch (message.type) {
        case "render": render(message); break;
        case "sessions": state.sessions = message; renderSessions(); break;
        case "banner": bannerEl.textContent = message.text; bannerEl.hidden = false; break;
        case "toast": toast(message.text); break;
        default: console.warn("unknown message", message);
      }
    },
  };

  setMode("normal");
  send({ type: "ready" });
})();
