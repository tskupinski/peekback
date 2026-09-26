(() => {
  const $ = (id) => document.getElementById(id);
  const docEl = $("doc");
  const bannerEl = $("banner");
  const noDocumentsEl = $("no-documents");
  const noDocumentsText = noDocumentsEl.textContent.trim();
  const composerEl = $("composer");
  const composerQuote = $("composer-quote");
  const composerInput = $("composer-input");
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
  const pickerEl = $("picker");
  const pickerInput = $("picker-input");
  const pickerList = $("picker-list");
  const pickerScope = $("picker-scope");
  const pickerCurrent = $("picker-current");
  const pickerScratchpad = $("picker-scratchpad");
  const pickerBookmarks = $("picker-bookmarks");
  const pickerNote = $("picker-note");
  const commentsEl = $("comments");
  const sendAllButton = $("send-all");

  const darkMode = () => window.matchMedia("(prefers-color-scheme: dark)").matches;

  // ---------------------------------------------------------------- state

  const state = {
    doc: null, // { path, source, label, session, sessionId, documents }; path is null before a session's first Markdown
    lastRender: null, // the render message, so the page can re-render itself
    blocks: [],
    cursor: 0,
    mode: "normal", // normal | visual | search | command | comment | picker
    anchor: null, // visual selection start, a block index
    pendingKey: null,
    count: "",
    search: { query: "", matches: [], index: -1 },
    comments: [], // { id, path, label, quote, note, line, endLine, anchorSource, anchorCount, anchorIndex, changed }
    confirm: null, // the question being asked: { text, choices }
    questions: [], // questions waiting their turn
    pendingAnswer: null, // a key answering the question while blocks render
    pendingJump: null, // { path, line } to apply once that document renders
  };

  // ------------------------------------------------------------- rendering

  // Documents are untrusted: an agent wrote them, or they came with a cloned
  // repo. Raw HTML stays off, and the CSP in index.html backs that up.
  const md = window
    .markdownit({
      html: false,
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

  function mermaidDefaults() {
    return { startOnLoad: false, securityLevel: "strict", theme: darkMode() ? "dark" : "default" };
  }
  let mermaidOptions = mermaidDefaults();
  mermaid.initialize(mermaidOptions);
  // Follows the system until the daemon names a theme.
  document.documentElement.dataset.theme = darkMode() ? "dark" : "light";

  let renderGeneration = 0;
  let previousOwner = null;
  let rendering = false;
  let nextRequestId = 1;
  const sameContext = (a, b) => JSON.stringify(a) === JSON.stringify(b);
  // Comments belong to the session, not to one run of its agent, so a resume
  // keeps them; sending still checks the exact run.
  const commentOwner = () => JSON.stringify(state.doc?.context?.session?.key ?? null);
  const activeComments = () => state.comments.filter(c => c.owner === commentOwner());

  async function render(message) {
    const generation = ++renderGeneration;
    const samePath = state.doc?.path === message.path;
    const sourceChanged = !samePath || state.doc.source !== message.source;
    const scrollY = samePath ? window.scrollY : 0;
    const previousLine = samePath ? state.blocks[state.cursor]?.dataset.sourceLine : undefined;

    rendering = true;
    state.blocks = [];
    const contextChanged = !sameContext(state.doc?.context, message.context);
    if (contextChanged) {
      for (const listing of [scratchpad, bookmarks]) Object.assign(listing, { documents: [], request_id: null, loading: false });
    }
    state.lastRender = message;
    state.doc = {
      path: message.path,
      fileIdentity: message.file_identity ?? message.path,
      context: message.context,
      source: message.source,
      session: message.session ?? null,
      sessionId: message.session?.session_id ?? null,
      documents: message.documents,
      label: labelFor(message.path, message.documents),
    };
    // Another session's comments wait for it; coming back finds them.
    const restored = previousOwner !== commentOwner() ? activeComments().length : 0;
    previousOwner = commentOwner();
    if (restored) toast(`${restored} unsent comment${restored === 1 ? "" : "s"} restored`);
    // A live reload starts a new view too; an open listing is asked for again
    // under it rather than left empty.
    if (contextChanged && !pickerEl.hidden && picker.kind === "documents") refreshScope();
    document.title = `${state.doc.label} - peekback`;
    bannerEl.hidden = true;
    noDocumentsEl.hidden = message.path !== null;
    noDocumentsEl.textContent = message.unstarted
      ? `This ${message.unstarted} session starts when you send its first prompt. Its documents open here then.`
      : noDocumentsText;

    const { frontmatter, body, offset } = splitFrontmatter(message.source);
    docEl.innerHTML = md.render(body);
    hoistFenceAttributes();
    if (offset) {
      for (const el of docEl.querySelectorAll("[data-source-line]")) {
        el.dataset.sourceLine = String(Number(el.dataset.sourceLine) + offset);
        el.dataset.sourceEnd = String(Number(el.dataset.sourceEnd) + offset);
      }
    }
    if (frontmatter) {
      const pre = document.createElement("pre");
      pre.className = "frontmatter";
      pre.textContent = frontmatter;
      pre.dataset.sourceLine = "0";
      pre.dataset.sourceEnd = String(offset);
      docEl.prepend(pre);
    }
    await renderMermaid();
    if (generation !== renderGeneration) return;
    renderMathInElement(docEl, {
      delimiters: [
        { left: "$$", right: "$$", display: true },
        { left: "$", right: "$", display: false },
      ],
      throwOnError: false,
      trust: false,
      maxSize: 100,
    });
    window.scrollTo(0, scrollY);

    collectBlocks();
    rendering = false;
    renderComments();
    placeComposer();
    if (state.pendingEdit) {
      const comment = activeComments().find((c) => c.id === state.pendingEdit);
      if (comment?.fileIdentity === state.doc.fileIdentity) {
        state.pendingEdit = null;
        Promise.resolve().then(() => openComposer({ editing: comment }));
      }
    }
    if (state.pendingAnswer && state.confirm) {
      const key = state.pendingAnswer;
      state.pendingAnswer = null;
      Promise.resolve().then(() => answer(key));
    }
    if (sourceChanged && state.anchor !== null) setMode("normal");
    if (state.pendingJump?.path === message.path) {
      state.cursor = nearestBlock(state.pendingJump.line);
      state.pendingJump = null;
    } else {
      state.cursor = previousLine === undefined ? 0 : nearestBlock(Number(previousLine));
    }
    if (state.search.query) runSearch(state.search.query, { keepIndex: true });
    paintCursor(false);
    renderDocuments();
    if (!pickerEl.hidden) updatePickerItems();
    renderStatus();
  }

  function rerender() {
    if (state.lastRender) render(state.lastRender);
  }

  // markdown-it puts a fence's attributes on the <code>, but the cursor
  // works on top-level elements, so the <pre> needs them.
  function hoistFenceAttributes() {
    for (const code of docEl.querySelectorAll("pre > code[data-source-line]")) {
      const pre = code.parentElement;
      pre.dataset.sourceLine = code.dataset.sourceLine;
      pre.dataset.sourceEnd = code.dataset.sourceEnd;
    }
  }

  // YAML frontmatter would otherwise render as a setext heading. Source
  // lines of the body are shifted so block selection still maps correctly.
  function splitFrontmatter(source) {
    const match = source.match(/^---\r?\n([\s\S]*?)\r?\n---\r?\n/);
    if (!match || !/^[\w-]+\s*:/.test(match[1])) return { frontmatter: null, body: source, offset: 0 };
    const offset = match[0].split("\n").length - 1;
    return { frontmatter: match[1], body: source.slice(match[0].length), offset };
  }

  function labelFor(path, documents) {
    if (path === null) return "no Markdown yet";
    return documents.find((d) => d.path === path)?.label ?? path.split("/").pop();
  }

  async function renderMermaid() {
    const nodes = [];
    for (const code of docEl.querySelectorAll("pre > code.language-mermaid")) {
      const pre = code.parentElement;
      const container = document.createElement("pre");
      container.className = "mermaid";
      container.textContent = code.textContent;
      container.dataset.sourceLine = pre.dataset.sourceLine;
      container.dataset.sourceEnd = pre.dataset.sourceEnd;
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

  // The terminal's palette and font, when the daemon could read them. Body
  // text keeps the system font; the terminal font goes on code and chrome.
  // The daemon names a built-in theme, or "auto" to follow the system
  // between light and dark; every color comes from that theme's stylesheet.
  let themeName = "auto";

  function applyTheme(theme) {
    themeName = theme?.name ?? "auto";
    const root = document.documentElement;
    if (theme?.font_family) root.style.setProperty("--mono", `"${theme.font_family}", ui-monospace, Menlo, monospace`);
    else root.style.removeProperty("--mono");
    if (theme?.font_size) root.style.setProperty("--mono-size", `${theme.font_size}px`);
    else root.style.removeProperty("--mono-size");
    showTheme();
  }

  function showTheme() {
    const root = document.documentElement;
    root.dataset.theme = themeName === "auto" ? (darkMode() ? "dark" : "light") : themeName;
    mermaidOptions = mermaidTheme();
    mermaid.initialize(mermaidOptions);
    rerender();
  }

  // Diagrams take the theme's colors, so they never look like another app's.
  function mermaidTheme() {
    const token = (name) => getComputedStyle(document.documentElement).getPropertyValue(`--${name}`).trim();
    const [bg, fg, accent, annotate] = ["bg", "fg", "accent", "annotate"].map(token);
    if (!bg) return mermaidDefaults();
    return {
      startOnLoad: false,
      securityLevel: "strict",
      theme: "base",
      themeVariables: {
        darkMode: getComputedStyle(document.documentElement).colorScheme === "dark",
        background: bg,
        primaryColor: mixHex(accent, bg, 0.16),
        primaryTextColor: fg,
        primaryBorderColor: accent,
        lineColor: token("muted"),
        secondaryColor: mixHex(annotate, bg, 0.16),
        tertiaryColor: token("code-bg"),
        fontFamily: token("font") || "sans-serif",
      },
    };
  }

  // Mermaid parses only plain colors, not CSS color-mix(); theme tokens are
  // #rrggbb, as tests/themes.js requires.
  function mixHex(a, b, t) {
    const parts = (hex) => [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16));
    const [x, y] = [parts(a), parts(b)];
    return "#" + x.map((v, i) => Math.round(v * t + y[i] * (1 - t)).toString(16).padStart(2, "0")).join("");
  }

  window.matchMedia("(prefers-color-scheme: dark)").addEventListener("change", () => {
    if (themeName === "auto") showTheme();
  });

  // ---------------------------------------------------------------- blocks

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

  function blockIndexOf(element) {
    const block = element?.closest?.("[data-source-line]");
    if (!block) return -1;
    return state.blocks.findIndex((b) => b === block || b.contains(block));
  }

  // The anchor outlives visual mode while the command line or the note
  // input is open, so `:c note` and `c` act on the whole selection.
  function selectionRange() {
    if (state.anchor !== null) {
      return [Math.min(state.anchor, state.cursor), Math.max(state.anchor, state.cursor)];
    }
    return [state.cursor, state.cursor];
  }

  function paintCursor(scroll = true) {
    const [from, to] = selectionRange();
    state.blocks.forEach((block, i) => {
      block.classList.toggle("cursor", i === state.cursor);
      block.classList.toggle("selected", state.anchor !== null && i >= from && i <= to);
    });
    const current = state.blocks[state.cursor];
    if (scroll && current) current.scrollIntoView({ block: "nearest" });
  }

  // map[1] from markdown-it is an exclusive end line.
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
    const index = state.blocks.findIndex((b) => b.getBoundingClientRect().bottom > 8);
    return index < 0 ? Math.max(0, state.blocks.length - 1) : index;
  }

  function scrollPage(fraction) {
    window.scrollBy({ top: fraction * window.innerHeight });
    requestAnimationFrame(() => {
      if (state.blocks.length === 0) return;
      state.cursor = topVisibleBlock();
      paintCursor(false);
    });
  }

  const halfPage = (direction) => scrollPage(direction / 2);
  const fullPage = (direction) => scrollPage(direction * 0.9);

  function nextHeading(direction) {
    const isHeading = (b) => /^H[1-6]$/.test(b.tagName);
    let i = state.cursor + direction;
    while (i >= 0 && i < state.blocks.length) {
      if (isHeading(state.blocks[i])) return moveCursor(i);
      i += direction;
    }
  }

  function scrollCursorTo(where) {
    const block = state.blocks[state.cursor];
    if (block) block.scrollIntoView({ block: where });
  }

  // ----------------------------------------------------------------- modes

  function setMode(mode) {
    // Only an answer ends a question; a reload or a click must not hide it.
    if (state.confirm && mode !== "confirm") {
      if (mode === "normal") state.anchor = null;
      paintCursor(false);
      return;
    }
    state.mode = mode;
    if (mode === "normal") state.anchor = null;
    modeEl.textContent = mode.toUpperCase();
    modeEl.dataset.mode = mode;
    paintCursor(false);
  }

  function leaveVisual() {
    if (state.anchor !== null) setMode("normal");
  }

  function setMessage(text) {
    statusMsgEl.textContent = text;
  }

  function sessionName(s) {
    const project = s.cwd.split("/").pop() || s.cwd;
    return `${project} · ${s.agent_name}`;
  }

  function renderStatus() {
    const parts = [];
    const session = state.doc?.session;
    if (session) parts.push(sessionName(session));
    if (state.doc) parts.push(state.doc.label);
    if (activeComments().length) parts.push(`${activeComments().length} pending`);
    statusDocEl.textContent = parts.join(" › ");
  }

  // The sidebar is off by default; the popup is narrow and the picker
  // covers the same ground.
  function setSidebar(visible) {
    document.body.classList.toggle("no-sidebar", !visible);
    try {
      localStorage.setItem("sidebar", visible ? "1" : "0");
    } catch (_) {}
  }
  function sidebarVisible() {
    return !document.body.classList.contains("no-sidebar");
  }
  try {
    setSidebar(localStorage.getItem("sidebar") === "1");
  } catch (_) {
    setSidebar(false);
  }

  // --------------------------------------------------------------- actions

  function switchTo(sessionId, path) {
    if (state.confirm) return;
    post({ type: "switch", session_id: sessionId, path });
  }

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
    perform(action, text, Number(state.blocks[from].dataset.sourceLine), Number(state.blocks[to].dataset.sourceEnd));
    if (action !== "comment") leaveVisual();
  }

  function perform(action, text, line, endLine) {
    if (rendering || state.confirm || !text) return;
    if (action === "copy") post({ type: "copy", text });
    if (action === "send") {
      const quoted = [lineRef(state.doc?.path, line, endLine), blockquote(text)].filter(Boolean).join("\n");
      const attached = commentsWithin(line, endLine);
      if (attached.length) sendComments(attached, `${quoted}\n\n`);
      else post({ type: "send", text: quoted, purpose: "selection" });
    }
    if (action === "comment") openComposer({ target: commentTarget(text, line, endLine) });
  }

  // -------------------------------------------------------------- comments

  // Comments belong to their session and live in memory until they are sent
  // or discarded, their session ends, or the daemon stops. Moving to another
  // session parks them.
  let nextCommentId = 1;
  let commentsInFlight = null; // { request_id, ids, after } for a send that has not reported back
  // Tells the agent a quote is no longer in the file, so it is not read as a
  // request to restore it.
  const CHANGED_NOTE = "(This text has since changed or been removed.)";

  function commentTarget(quote, line, endLine) {
    if (!state.doc) return null;
    return {
      path: state.doc.path,
      fileIdentity: state.doc.fileIdentity,
      owner: commentOwner(),
      label: promptPathFor(state.doc.path),
      quote,
      line,
      endLine,
      anchorSource: Number.isInteger(line) && Number.isInteger(endLine)
        ? state.doc.source.split("\n").slice(line, endLine).join("\n") : null,
      ...anchorOrder(line, endLine),
    };
  }

  // ------------------------------------------------------------ composer

  // Writing a note happens under the block it is about. It lives outside the
  // rendered document, so a live reload while typing keeps the draft.
  let composing = null; // { target } for a new comment, { editing } for an existing one

  function openComposer(what) {
    if (!state.doc || (!what.target?.quote && !what.editing)) return;
    closePicker();
    closeCmdline();
    composing = what;
    const { quote, note } = what.editing ?? { quote: what.target.quote, note: "" };
    composerQuote.textContent = firstLine(quote);
    composerInput.value = note;
    composerEl.hidden = false;
    setMode("comment");
    fitComposer();
    placeComposer();
    composerInput.focus();
  }

  function closeComposer() {
    if (composerEl.hidden) return;
    composerEl.hidden = true;
    composing = null;
    composerInput.blur();
    leaveVisual();
    setMode("normal");
    showQuestionLater();
  }

  function saveComposer() {
    const what = composing;
    const note = composerInput.value;
    closeComposer();
    if (what?.editing) {
      what.editing.note = note.trim();
      renderComments();
      setMessage("comment updated");
    } else if (what?.target) {
      addComment(what.target, note);
    }
  }

  // Under the last block the note is about, where the eye already is.
  function placeComposer() {
    if (composerEl.hidden || !composing) return;
    const { line, endLine } = composing.editing ?? composing.target;
    const block = state.blocks[nearestBlock(Number.isInteger(endLine) ? endLine - 1 : line)];
    if (!block) return;
    const rect = block.getBoundingClientRect();
    const column = docEl.getBoundingClientRect();
    composerEl.style.top = `${rect.bottom + (window.scrollY || 0) + 8}px`;
    composerEl.style.left = `${rect.left}px`;
    composerEl.style.width = `${Math.max(240, Math.min(640, column.right - rect.left - 24))}px`;
    composerEl.scrollIntoView({ block: "nearest" });
  }

  function fitComposer() {
    composerInput.rows = Math.min(8, Math.max(2, composerInput.value.split("\n").length));
  }

  function firstLine(quote) {
    const line = quote.split("\n").find((l) => l.trim()) ?? "";
    return line.replace(/^\s*(#{1,6}|>|[-*+]|\d+[.)])\s+/, "").slice(0, 120);
  }

  composerInput.addEventListener("input", fitComposer);
  composerInput.addEventListener("keydown", (event) => {
    event.stopPropagation();
    if (event.isComposing) return;
    if (event.key === "Escape") {
      event.preventDefault();
      closeComposer();
    } else if (event.key === "Enter" && !event.shiftKey) {
      event.preventDefault();
      saveComposer();
    }
  });

  // The comment on the cursor's block; with several, the latest.
  function editCommentAtCursor() {
    const block = state.blocks[state.cursor];
    if (!block) return;
    const start = Number(block.dataset.sourceLine);
    const end = Number(block.dataset.sourceEnd);
    const here = activeComments().filter((c) => !c.changed && c.fileIdentity === state.doc?.fileIdentity
      && c.line < end && c.endLine > start);
    if (here.length === 0) return setMessage("no comment on this block · c adds one");
    openComposer({ editing: here.at(-1) });
    if (here.length > 1) setMessage(`${here.length} comments here · Space c lists them`);
  }

  function editComment(c) {
    if (c.fileIdentity !== state.doc?.fileIdentity) {
      state.pendingEdit = c.id;
      return jumpToComment(c);
    }
    openComposer({ editing: c });
  }

  function addComment(target, note) {
    if (!target?.quote) return;
    state.comments.push({
      ...target,
      id: nextCommentId++,
      note: note.trim(),
    });
    renderComments();
    setMessage(`${activeComments().length} pending comment${activeComments().length === 1 ? "" : "s"}`);
  }

  function removeComment(id) {
    state.comments = state.comments.filter((c) => c.id !== id);
    renderComments();
  }

  // Relative to the session's cwd when under it, otherwise absolute, since
  // the agent will read this path.
  function promptPathFor(path) {
    const session = state.doc?.session;
    if (session && path.startsWith(session.cwd + "/")) return path.slice(session.cwd.length + 1);
    return path;
  }

  function commentsPrompt(comments) {
    const byPath = new Map();
    for (const c of comments) {
      if (!byPath.has(c.fileIdentity)) byPath.set(c.fileIdentity, []);
      byPath.get(c.fileIdentity).push(c);
    }
    return [...byPath.entries()]
      .map(([path, group]) => {
        const label = promptPathFor(path);
        const body = group
          .map((c) => [c.changed ? null : lineRef(path, c.line, c.endLine), blockquote(c.quote),
            c.changed ? CHANGED_NOTE : null, c.note].filter(Boolean).join("\n"))
          .join("\n\n");
        return `Comments on \`${label}\`:\n\n${body}`;
      })
      .join("\n\n");
  }

  // Where a quote sits in the file, as `path#L12-18`, so the agent finds it
  // without searching, even after it has edited the file.
  function lineRef(path, line, endLine) {
    if (!path || !Number.isInteger(line) || !Number.isInteger(endLine) || endLine <= line) return null;
    const first = line + 1;
    return `\`${promptPathFor(path)}#L${first}${endLine > first ? `-${endLine}` : ""}\``;
  }

  function sendAllComments(after) {
    const comments = activeComments();
    if (state.confirm) return after?.kept();
    if (rendering) {
      setMessage("still rendering; try again");
      return after?.kept();
    }
    // With nothing left unsent there is nothing to keep the viewer here for.
    if (comments.length === 0) {
      after?.sent();
      return setMessage("no pending comments for this session");
    }
    sendComments(comments, "", after);
  }

  // `after` hears whether the comments went out (sent) or stayed (kept).
  function sendComments(comments, prefix, after) {
    if (commentsInFlight) {
      after?.kept();
      return setMessage("still sending the previous batch");
    }
    const request_id = post({ type: "send", text: prefix + commentsPrompt(comments), purpose: "comments" });
    if (request_id === null) return after?.kept();
    commentsInFlight = { request_id, ids: comments.map((c) => c.id), after };
  }

  // Parked comments go with their session when it ends; the one on screen
  // stays, since its documents remain readable.
  function dropEnded(sessions) {
    const live = new Set(sessions.map((key) => JSON.stringify(key)));
    state.comments = state.comments.filter((c) => c.owner === commentOwner() || live.has(c.owner));
  }

  function markSent(ids) {
    const sent = new Set(ids);
    state.comments = state.comments.filter((c) => !sent.has(c.id));
    renderComments();
  }

  function onSendResult(message) {
    toast(message.text);
    if (message.purpose !== "comments" || message.request_id !== commentsInFlight?.request_id) return;
    const { ids, after } = commentsInFlight;
    commentsInFlight = null;
    if (message.ok && message.outcome === "pasted") {
      markSent(ids);
      after?.sent();
    } else if (message.ok && message.outcome === "copied") {
      // Only you know whether it reached the prompt; keeping them unasked
      // would send them again next time.
      const n = ids.length;
      ask(`Copied ${n} comment${n === 1 ? "" : "s"} to the clipboard. Pasted ${n === 1 ? "it" : "them"}? y marks sent · n keeps`, {
        y: () => { markSent(ids); after?.sent(); },
        n: () => after?.kept(),
      });
    } else {
      after?.kept();
    }
  }

  // Comments whose text lies wholly inside a selection travel with it; one
  // reaching beyond it was about more than you chose to send.
  function commentsWithin(line, endLine) {
    if (!Number.isInteger(line) || !Number.isInteger(endLine)) return [];
    return activeComments().filter((c) => !c.changed && c.fileIdentity === state.doc?.fileIdentity
      && c.line >= line && c.endLine <= endLine);
  }

  // One-key questions in the status line, asked one at a time and each
  // answered: a newer one waits its turn, since dropping "Pasted them?"
  // would send those comments again. Escape answers with the question's own
  // Escape, else n. A question never interrupts typing: while an input is
  // open it waits until the input closes, so a note being written is kept.
  function ask(text, choices) {
    state.questions.push({ text, choices });
    showQuestion();
  }

  function showQuestion() {
    if (state.confirm || state.questions.length === 0) return;
    if (["command", "search", "comment", "picker"].includes(state.mode)) return;
    state.confirm = state.questions.shift();
    setMode("confirm");
    const { text } = state.confirm;
    setMessage(typeof text === "function" ? text() : text);
  }

  // After the key that closed an input has done its work, so its own message
  // does not replace the question.
  function showQuestionLater() {
    Promise.resolve().then(showQuestion);
  }

  function answer(key) {
    // An answer given mid-render waits for the blocks; sending first would
    // otherwise be refused and look like it had happened.
    if (rendering) {
      state.pendingAnswer = key;
      return;
    }
    const { choices } = state.confirm;
    const choice = choices[key] ?? (key === "Escape" ? choices.Escape ?? choices.n : null);
    if (!choice) return;
    state.confirm = null;
    setMode("normal");
    setMessage("");
    choice();
    showQuestion();
  }

  function renderComments() {
    paintComments();
    commentsEl.replaceChildren(
      ...activeComments().map((c) => {
        const li = document.createElement("li");
        li.className = "comment";
        const quote = document.createElement("div");
        quote.className = "comment-quote";
        quote.textContent = c.quote;
        const note = document.createElement("div");
        note.className = "comment-note";
        note.textContent = c.note || "(no note)";
        const changed = document.createElement("div");
        changed.className = "comment-changed";
        changed.textContent = "text changed or removed";
        changed.hidden = !c.changed;
        const remove = document.createElement("button");
        remove.className = "comment-remove";
        remove.textContent = "×";
        remove.title = "Remove";
        remove.addEventListener("click", (event) => {
          event.stopPropagation();
          removeComment(c.id);
        });
        li.append(quote, changed, note, remove);
        li.addEventListener("click", () => jumpToComment(c));
        return li;
      })
    );
    sendAllButton.hidden = activeComments().length === 0;
    sendAllButton.textContent = `Send all (${activeComments().length})`;
    renderStatus();
  }

  // The daemon asks before a session switch would drop these.
  function paintComments() {
    const lines = state.doc?.source.split("\n") ?? [];
    const here = activeComments().filter((c) => c.fileIdentity === state.doc?.fileIdentity && c.anchorSource !== null);
    for (const c of here) c.changed = !locate(c, lines);
    const comments = here.filter((c) => !c.changed);
    for (const block of state.blocks) {
      const start = Number(block.dataset.sourceLine);
      const end = Number(block.dataset.sourceEnd);
      const attached = comments.filter((c) => c.line < end && c.endLine > start);
      block.classList.toggle("commented", attached.length > 0);
      if (attached.length) {
        block.dataset.commentLabel = `${attached.length} comment${attached.length === 1 ? "" : "s"}`;
        block.setAttribute("title", attached.map((c) => c.note || "(no note)").join("\n\n"));
      } else {
        delete block.dataset.commentLabel;
        block.removeAttribute("title");
      }
    }
  }

  // Follows text that moved, as when the agent adds a paragraph above it.
  // Anything else is reported as changed rather than guessed at.
  // Only when the text occurs as often as when you commented, and then the
  // same copy in order: with one copy gone, such as a repeated heading,
  // there is no telling which was yours.
  function locate(c, lines) {
    const found = occurrences(lines, c.anchorSource);
    if (found.length === 0 || found.length !== c.anchorCount) return false;
    const at = found[c.anchorIndex];
    [c.line, c.endLine] = [at, at + (c.endLine - c.line)];
    return true;
  }

  // Which copy of its text a new comment is on, among how many.
  function anchorOrder(line, endLine) {
    if (!Number.isInteger(line) || !Number.isInteger(endLine)) return { anchorCount: 0, anchorIndex: -1 };
    const lines = state.doc.source.split("\n");
    const found = occurrences(lines, lines.slice(line, endLine).join("\n"));
    return { anchorCount: found.length, anchorIndex: found.indexOf(line) };
  }

  function occurrences(lines, anchor) {
    const want = anchor.split("\n");
    const found = [];
    for (let i = 0; i + want.length <= lines.length; i++) {
      if (lines[i] === want[0] && lines.slice(i, i + want.length).join("\n") === anchor) found.push(i);
    }
    return found;
  }

  function jumpToComment(c) {
    if (c.path !== state.doc?.path) {
      state.pendingJump = { path: c.path, line: c.line };
      switchTo(state.doc?.sessionId ?? null, c.path);
      return;
    }
    if (c.changed) return setMessage("that text has changed or been removed");
    if (c.line !== undefined && c.line !== null) moveCursor(nearestBlock(c.line));
  }

  function commentCandidates() {
    return activeComments().map((c) => ({
      label: `${c.quote.split("\n")[0].slice(0, 60)}${c.note ? "  ·  " + c.note : ""}`,
      meta: c.changed ? `${c.label} · text changed or removed` : c.label,
      id: c.id,
      run: () => jumpToComment(c),
    }));
  }

  sendAllButton.addEventListener("click", () => sendAllComments());

  // ---------------------------------------------------------------- search

  const highlightsSupported = "highlights" in CSS && typeof Highlight === "function";

  function runSearch(query, { keepIndex = false } = {}) {
    const previousIndex = state.search.index;
    state.search.query = query;
    state.search.matches = [];
    state.search.index = -1;
    if (highlightsSupported) CSS.highlights.delete("search");
    state.blocks.forEach((b) => b.classList.remove("match"));
    if (!query) return;

    const needle = query.toLowerCase();
    const highlight = highlightsSupported ? new Highlight() : null;
    const walker = document.createTreeWalker(docEl, NodeFilter.SHOW_TEXT);
    for (let node = walker.nextNode(); node; node = walker.nextNode()) {
      const text = node.textContent.toLowerCase();
      // Case folding can change string length; offsets would then be wrong.
      if (text.length !== node.textContent.length) continue;
      let at = text.indexOf(needle);
      while (at >= 0) {
        const range = new Range();
        range.setStart(node, at);
        range.setEnd(node, at + needle.length);
        highlight?.add(range);
        const block = blockIndexOf(node.parentElement);
        if (!highlightsSupported && block >= 0) state.blocks[block].classList.add("match");
        state.search.matches.push({ range, block });
        at = text.indexOf(needle, at + needle.length);
      }
    }
    if (highlight) CSS.highlights.set("search", highlight);
    const count = state.search.matches.length;
    if (keepIndex && previousIndex >= 0 && previousIndex < count) state.search.index = previousIndex;
    setMessage(count ? `${count} match${count === 1 ? "" : "es"}` : "no matches");
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

  // -------------------------------------------------------------- commands

  const commands = {
    q: () => post({ type: "hide" }),
    quit: () => post({ type: "hide" }),
    doc: (arg) => (arg ? switchByName(documentCandidates(), arg, "document") : openPicker("documents")),
    send: () => actOnSelection("send"),
    copy: () => actOnSelection("copy"),
    c: (arg) => {
      if (!state.blocks.length) return;
      if (!arg) return actOnSelection("comment");
      const [from, to] = selectionRange();
      addComment(commentTarget(sourceOf(from, to), Number(state.blocks[from].dataset.sourceLine),
        Number(state.blocks[to].dataset.sourceEnd)), arg);
      leaveVisual();
    },
    sendall: () => sendAllComments(),
    comments: () => openPicker("comments"),
    help: () => toggleHelp(),
    sidebar: () => setSidebar(!sidebarVisible()),
  };

  function documentCandidates() {
    if (picker.scope === "bookmarks") {
      return bookmarks.documents.map((d) => ({
        label: d.label,
        search: `${d.label} ${d.path}`,
        meta: d.touched_at ? `edited ${ago(d.touched_at)}` : "",
        current: d.path === state.doc?.path,
        run: () => post({ type: "open-bookmark", path: d.path, request_id: bookmarks.request_id }),
      }));
    }
    if (picker.scope === "scratchpad") {
      return scratchpad.documents.map((d) => ({
        label: d.label,
        meta: d.touched_at ? ago(d.touched_at) : "",
        current: d.path === state.doc?.path,
        run: () => post({ type: "open-scratchpad", path: d.path, request_id: scratchpad.request_id }),
      }));
    }
    if (!state.doc) return [];
    return state.doc.documents.map((d) => ({
      label: d.label,
      meta: [d.touched_at ? ago(d.touched_at) : "", evidenceNote(d)].filter(Boolean).join(" · "),
      current: d.path === state.doc.path,
      run: () => switchTo(state.doc.sessionId, d.path),
    }));
  }

  // No tool reported these writes; a scan at the end of a turn found them.
  function evidenceNote(d) {
    if (d.shared) return "found by scan, maybe another session's";
    return d.scanned ? "found by scan" : "";
  }

  function switchByName(candidates, arg, what) {
    const match = fuzzy(candidates, arg)[0];
    if (!match) return setMessage(`no ${what} matching ${arg}`);
    match.run();
  }

  function fuzzy(candidates, query) {
    const q = (query || "").toLowerCase();
    if (!q) return candidates;
    return candidates
      .map((c) => ({ c, score: fuzzyScore((c.search ?? c.label).toLowerCase(), q) }))
      .filter((x) => x.score >= 0)
      .sort((a, b) => b.score - a.score)
      .map((x) => x.c);
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
    const match = line.trim().match(/^(\S+)\s*([\s\S]*)$/);
    if (!match) return;
    const [, name, arg] = match;
    const handler = commands[name];
    if (!handler) return setMessage(`unknown command :${name}`);
    handler(arg);
  }

  // The command line doubles as the search field; `cmdPrefix` tells them
  // apart.
  let cmdPrefix = ":";
  let completionList = [];
  let completionIndex = -1;

  function openCmdline(prefix) {
    cmdPrefix = prefix;
    cmdPrefixEl.textContent = prefix;
    cmdInput.value = "";
    cmdInput.placeholder = "";
    cmdlineEl.hidden = false;
    hideCompletions();
    setMode(prefix === ":" ? "command" : "search");
    cmdInput.focus();
  }

  function closeCmdline() {
    if (cmdlineEl.hidden) return;
    cmdlineEl.hidden = true;
    hideCompletions();
    cmdInput.blur();
    setMode(state.anchor !== null ? "visual" : "normal");
    showQuestionLater();
  }

  function completionsFor(value) {
    const [name, ...rest] = value.split(/\s+/);
    const arg = rest.join(" ");
    if (rest.length === 0) {
      return Object.keys(commands)
        .filter((c) => c.startsWith(name))
        .map((c) => ({ label: c, apply: () => `${c} ` }));
    }
    const source = name === "doc" ? documentCandidates() : [];
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
    else if (cmdPrefix === ":") showCompletions();
  });

  cmdInput.addEventListener("keydown", (event) => {
    event.stopPropagation();
    if (event.key === "Escape") {
      if (cmdPrefix === "/") clearSearch();
      closeCmdline();
    } else if (event.key === "Enter") {
      const value = cmdInput.value;
      const prefix = cmdPrefix;
      closeCmdline();
      if (prefix === "/") gotoMatch(1);
      else runCommand(value);
    } else if (event.key === "Tab") {
      event.preventDefault();
      if (cmdPrefix === ":") cycleCompletion(event.shiftKey ? -1 : 1);
    }
  });

  // Clicking the document while an input is open would otherwise leave a
  // mode nobody can leave by keyboard.
  cmdInput.addEventListener("blur", () => {
    if (document.hasFocus()) closeCmdline();
  });

  function toggleHelp() {
    helpEl.hidden = !helpEl.hidden;
  }

  // ---------------------------------------------------------------- picker

  // An fzf-style overlay: type to filter, Enter to open. Replaces the
  // sidebar as the way to move between documents.
  const picker = { kind: null, scope: "current", items: [], filtered: [], index: 0 };
  const scratchpad = { documents: [], found: 0, warnings: [], loading: false, available: true };
  const bookmarks = { documents: [], found: 0, warnings: [], loading: false };
  const SCOPES = ["current", "scratchpad", "bookmarks"];
  const BOOKMARKS_HINT = 'Add bookmarks = ["~/.claude/CLAUDE.md", "CLAUDE.md"] to ~/.config/peekback/config.toml. '
    + "Entries can be files, folders or globs; ones not starting with / or ~ follow the session's project.";

  // Read by the daemon on every request, so config edits apply on the next open.
  function refreshBookmarks() {
    bookmarks.loading = true;
    bookmarks.documents = [];
    bookmarks.request_id = post({ type: "list-bookmarks" });
  }

  // Listed live, so files show up as soon as they are written, mid-turn too.
  function refreshScratchpad() {
    // The previous list may belong to another session; never offer it meanwhile.
    Object.assign(scratchpad, { documents: [], found: 0, warnings: [], loading: true });
    scratchpad.request_id = post({ type: "list-scratchpad" });
  }

  function refreshScope() {
    if (picker.scope === "scratchpad") refreshScratchpad();
    if (picker.scope === "bookmarks") refreshBookmarks();
  }

  function updatePickerItems() {
    picker.items = picker.kind === "documents" ? documentCandidates() : commentCandidates();
    filterPicker();
  }

  function setPickerScope(scope) {
    picker.scope = scope;
    refreshScope();
    updatePickerItems();
    pickerInput.focus();
  }

  for (const [button, scope] of [[pickerCurrent, "current"], [pickerScratchpad, "scratchpad"], [pickerBookmarks, "bookmarks"]]) {
    button.addEventListener("mousedown", (event) => event.preventDefault());
    button.addEventListener("click", () => setPickerScope(scope));
  }

  function openPicker(kind) {
    picker.kind = kind;
    if (kind === "documents" && picker.scope !== "bookmarks" && !state.doc?.sessionId) picker.scope = "bookmarks";
    if (kind === "documents") refreshScope();
    pickerInput.value = "";
    pickerInput.placeholder = { documents: "open document", comments: "pending comments, Ctrl-E edits, Ctrl-D removes" }[kind];
    pickerEl.hidden = false;
    setMode("picker");
    updatePickerItems();
    pickerInput.focus();
  }

  function closePicker() {
    if (pickerEl.hidden) return;
    pickerEl.hidden = true;
    pickerInput.blur();
    setMode(state.anchor !== null ? "visual" : "normal");
    showQuestionLater();
  }

  function filterPicker() {
    picker.filtered = fuzzy(picker.items, pickerInput.value);
    picker.index = 0;
    renderPicker();
  }

  function renderPicker() {
    const documents = picker.kind === "documents";
    const scratch = documents && picker.scope === "scratchpad";
    const marked = documents && picker.scope === "bookmarks";
    pickerScope.hidden = !documents;
    pickerCurrent.setAttribute("aria-pressed", String(documents && picker.scope === "current"));
    pickerScratchpad.setAttribute("aria-pressed", String(scratch));
    pickerBookmarks.setAttribute("aria-pressed", String(marked));
    pickerNote.hidden = !scratch && !marked;
    const opens = state.doc?.sessionId ? "Opens in this session." : "Opens without a send target.";
    if (scratch) {
      const shown = scratchpad.found > scratchpad.documents.length
        ? `Showing ${scratchpad.documents.length} of ${scratchpad.found} files. ` : "";
      pickerNote.textContent = scratchpad.loading ? "Loading scratchpad…" : !scratchpad.available
        ? "No scratchpad: the session has ended, or it is not a Claude Code session."
        : scratchpad.warnings.length ? `${shown}${scratchpad.warnings.join("\n")}`
          : `${shown}Markdown in this session's scratchpad, newest first.`;
    } else if (marked) {
      const shown = bookmarks.found > bookmarks.documents.length
        ? `Showing ${bookmarks.documents.length} of ${bookmarks.found} bookmarked files. ` : "";
      pickerNote.textContent = bookmarks.loading ? "Loading bookmarks…" : bookmarks.warnings.length
        ? `${shown}Some bookmarks could not be listed:\n${bookmarks.warnings.join("\n")}`
        : `${shown}From ~/.config/peekback/config.toml. ${opens}`;
    }
    const start = Math.max(0, picker.index - 29);
    pickerList.replaceChildren(
      ...picker.filtered.slice(start, start + 30).map((c, offset) => {
        const i = start + offset;
        const li = document.createElement("li");
        li.className = "picker-item" + (i === picker.index ? " active" : "") + (c.current ? " current" : "");
        const name = document.createElement("span");
        name.className = "name";
        name.textContent = c.label;
        const meta = document.createElement("span");
        meta.className = "meta";
        meta.textContent = c.meta ?? "";
        li.title = `${c.label}\n${c.meta ?? ""}`;
        li.append(name, meta);
        li.addEventListener("mousedown", (event) => event.preventDefault());
        li.addEventListener("click", () => {
          picker.index = i;
          choosePicker();
        });
        return li;
      })
    );
    if (picker.filtered.length === 0) {
      const li = document.createElement("li");
      li.className = "empty";
      li.textContent = scratch && scratchpad.loading ? "loading…" : scratch && !scratchpad.available
        ? "No scratchpad for this session"
        : scratch && scratchpad.documents.length === 0 ? "No Markdown in this session's scratchpad yet"
        : marked && bookmarks.loading ? "loading…" : marked && bookmarks.documents.length === 0
          ? `No bookmarked Markdown files. ${BOOKMARKS_HINT}`
          : documents && state.doc?.sessionId && picker.items.length === 0 ? "This session has not written any Markdown yet" : "no matches";
      pickerList.append(li);
    }
  }

  function movePicker(step) {
    if (picker.filtered.length === 0) return;
    picker.index = (picker.index + step + picker.filtered.length) % picker.filtered.length;
    renderPicker();
    pickerList.querySelector(".active")?.scrollIntoView({ block: "nearest" });
  }

  function choosePicker() {
    const chosen = picker.filtered[picker.index];
    closePicker();
    if (chosen) chosen.run();
  }

  pickerInput.addEventListener("input", filterPicker);
  pickerInput.addEventListener("keydown", (event) => {
    event.stopPropagation();
    const ctrl = event.ctrlKey;
    if (event.key === "Escape") closePicker();
    else if (event.key === "Enter") choosePicker();
    else if (event.key === "Tab" && picker.kind === "documents") {
      event.preventDefault();
      const step = event.shiftKey ? SCOPES.length - 1 : 1;
      setPickerScope(SCOPES[(SCOPES.indexOf(picker.scope) + step) % SCOPES.length]);
    } else if (ctrl && event.key === "r" && picker.kind === "documents" && picker.scope !== "current") {
      event.preventDefault();
      refreshScope();
      updatePickerItems();
    }
    else if (event.key === "ArrowDown" || (ctrl && (event.key === "n" || event.key === "j"))) {
      event.preventDefault();
      movePicker(1);
    } else if (event.key === "ArrowUp" || (ctrl && (event.key === "p" || event.key === "k"))) {
      event.preventDefault();
      movePicker(-1);
    } else if (ctrl && event.key === "e" && picker.kind === "comments") {
      event.preventDefault();
      const chosen = picker.filtered[picker.index];
      const comment = chosen && activeComments().find((c) => c.id === chosen.id);
      closePicker();
      if (comment) editComment(comment);
    } else if (ctrl && event.key === "d" && picker.kind === "comments") {
      event.preventDefault();
      const chosen = picker.filtered[picker.index];
      if (chosen) {
        removeComment(chosen.id);
        picker.items = commentCandidates();
        filterPicker();
      }
    }
  });
  pickerInput.addEventListener("blur", () => {
    if (document.hasFocus()) closePicker();
  });

  // ------------------------------------------------------------------ keys

  const pendingTimeout = 800;
  let pendingTimer = null;

  function setPending(key) {
    state.pendingKey = key;
    clearTimeout(pendingTimer);
    pendingTimer = setTimeout(() => (state.pendingKey = null), pendingTimeout);
    setMessage(key === " " ? "space" : key);
  }

  function takePending() {
    const key = state.pendingKey;
    if (key !== null) {
      state.pendingKey = null;
      clearTimeout(pendingTimer);
      setMessage("");
    }
    return key;
  }

  function takeCount() {
    const n = state.count ? parseInt(state.count, 10) : 1;
    state.count = "";
    return n;
  }

  const modifierKeys = new Set(["Shift", "Control", "Alt", "Meta", "CapsLock"]);

  window.addEventListener("keydown", (event) => {
    if (state.confirm) {
      if (modifierKeys.has(event.key)) return;
      event.preventDefault();
      return answer(event.key);
    }
    if (rendering && event.key !== "Escape") return;
    // A click elsewhere took focus from the note; the next key brings it back.
    if (state.mode === "comment" && !composerEl.hidden) return composerInput.focus();
    if (["command", "search", "picker", "comment"].includes(state.mode)) return;
    if (modifierKeys.has(event.key)) return;
    if (event.ctrlKey && !event.metaKey && !event.altKey) {
      const ctrl = { p: () => openPicker("documents"), d: () => halfPage(1), u: () => halfPage(-1), f: () => fullPage(1), b: () => fullPage(-1) }[event.key];
      if (ctrl) {
        event.preventDefault();
        ctrl();
      }
      return;
    }
    if (event.metaKey || event.ctrlKey || event.altKey) return;
    if (!helpEl.hidden && event.key !== "?") {
      helpEl.hidden = true;
      if (event.key === "Escape") return;
    }
    const key = event.key;
    if (/^[0-9]$/.test(key) && !(key === "0" && state.count === "") && !state.pendingKey) {
      state.count += key;
      setMessage(state.count);
      event.preventDefault();
      return;
    }
    const pending = takePending();
    let handled = true;

    if (pending === "g") {
      if (key === "g") moveCursor(state.count ? takeCount() - 1 : 0);
      else handled = false;
    } else if (pending === "z") {
      if (key === "z") scrollCursorTo("center");
      else if (key === "t") scrollCursorTo("start");
      else if (key === "b") scrollCursorTo("end");
      else handled = false;
    } else if (pending === " ") {
      if (key === "d") openPicker("documents");
      else if (key === "c") openPicker("comments");
      else handled = false;
    } else if (pending === "]" || pending === "[") {
      const direction = pending === "]" ? 1 : -1;
      if (key === pending) nextHeading(direction);
      else if (key === "d") cycleDocument(direction);
      else handled = false;
    } else {
      switch (key) {
        case "j": moveCursor(state.cursor + takeCount()); break;
        case "k": moveCursor(state.cursor - takeCount()); break;
        case "d": halfPage(1); break;
        case "u": halfPage(-1); break;
        case "G": moveCursor(state.count ? takeCount() - 1 : state.blocks.length - 1); break;
        case "g": case "]": case "[": case "z": case " ": setPending(key); break;
        case "v":
          if (state.anchor !== null) setMode("normal");
          else {
            state.anchor = state.cursor;
            setMode("visual");
          }
          break;
        case "y": actOnSelection("copy"); break;
        case "s": actOnSelection("send"); break;
        case "c": actOnSelection("comment"); break;
        case "C": editCommentAtCursor(); break;
        case "S": sendAllComments(); break;
        case "/": openCmdline("/"); break;
        case ":": openCmdline(":"); break;
        case "n": gotoMatch(1); break;
        case "N": gotoMatch(-1); break;
        case "?": toggleHelp(); break;
        case "Tab": setSidebar(!sidebarVisible()); break;
        case "Escape":
          if (state.count) state.count = "";
          else if (state.anchor !== null) setMode("normal");
          else if (state.search.query) clearSearch();
          else post({ type: "hide" });
          break;
        default: handled = false;
      }
    }
    if (handled) event.preventDefault();
    else state.count = "";
  });

  function cycleDocument(direction) {
    if (!state.doc) return;
    const docs = state.doc.documents;
    const i = docs.findIndex((d) => d.path === state.doc.path);
    const next = docs[(i + direction + docs.length) % docs.length];
    if (next) switchTo(state.doc.sessionId, next.path);
  }

  // -------------------------------------------------------- mouse and links

  function mouseSelection() {
    const selection = window.getSelection();
    if (!selection || selection.isCollapsed || selection.rangeCount === 0) return null;
    const range = selection.getRangeAt(0);
    if (!docEl.contains(range.commonAncestorContainer)) return null;
    return { text: selection.toString().trim(), rect: range.getBoundingClientRect(), range };
  }

  function placeToolbar() {
    const found = mouseSelection();
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
    const found = mouseSelection();
    if (found) {
      // Range endpoints are in document order even for a backwards selection.
      const element = (node) => node.nodeType === Node.ELEMENT_NODE ? node : node.parentElement;
      const first = state.blocks[blockIndexOf(element(found.range.startContainer))];
      const last = state.blocks[blockIndexOf(element(found.range.endContainer))];
      perform(button.dataset.action, found.text,
        first ? Number(first.dataset.sourceLine) : undefined,
        last ? Number(last.dataset.sourceEnd) : undefined);
    }
    window.getSelection()?.removeAllRanges();
    toolbarEl.hidden = true;
  });

  docEl.addEventListener("click", (event) => {
    const index = blockIndexOf(event.target);
    if (index >= 0 && window.getSelection()?.isCollapsed) {
      state.cursor = index;
      paintCursor(false);
    }
  });

  document.addEventListener("click", (event) => {
    const link = event.target.closest("a");
    const href = link && linkTarget(link);
    if (href === null || href.startsWith("#")) return;
    event.preventDefault();
    if (/^https?:\/\//.test(href)) post({ type: "open-external", url: href });
    else if (/\.(md|markdown)$/i.test(href)) openRelativeLink(href);
    else setMessage("only web links and Markdown files open from here");
  });

  // Mermaid draws its links as SVG anchors, which carry xlink:href instead
  // of href. Either kind must go through the handler above, never the webview.
  function linkTarget(link) {
    return link.getAttribute("href") ?? link.getAttributeNS("http://www.w3.org/1999/xlink", "href");
  }

  // A link to another Markdown file resolves against the current document
  // and opens if the session lists it.
  function openRelativeLink(href) {
    if (!state.doc) return;
    const base = state.doc.path.split("/").slice(0, -1);
    for (const part of href.split("/")) {
      if (part === "..") base.pop();
      else if (part && part !== ".") base.push(part);
    }
    const target = base.join("/");
    if (state.doc.documents.some((d) => d.path === target)) switchTo(state.doc.sessionId, target);
    else setMessage(`${href} is not among this session's documents`);
  }

  // --------------------------------------------------------------- sidebar

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

  function renderDocuments() {
    if (!state.doc) return;
    documentsEl.replaceChildren(
      ...state.doc.documents.map((d) => {
        const slash = d.label.lastIndexOf("/");
        return item({
          name: slash >= 0 ? d.label.slice(slash + 1) : d.label,
          meta: slash >= 0 ? d.label.slice(0, slash) : "",
          title: [d.path, evidenceNote(d)].filter(Boolean).join("\n"),
          current: d.path === state.doc.path,
          onClick: () => switchTo(state.doc.sessionId, d.path),
        });
      })
    );
  }

  // ------------------------------------------------------------------ misc

  let toastTimer = null;
  function toast(text) {
    toastEl.textContent = text;
    toastEl.hidden = false;
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => (toastEl.hidden = true), 4000);
    setMessage(text);
  }

  // The daemon drops larger messages without a reply.
  const MAX_MESSAGE_BYTES = 4 * 1024 * 1024;

  function post(message) {
    if (["send", "switch", "list-bookmarks", "list-scratchpad", "open-bookmark", "open-scratchpad"].includes(message.type)) {
      message.context = state.doc?.context ?? { generation: 0, session: null };
      message.request_id ??= nextRequestId++;
    }
    const body = JSON.stringify(message);
    if (new TextEncoder().encode(body).length > MAX_MESSAGE_BYTES) {
      toast("Too large to send; select less text");
      return null;
    }
    window.ipc.postMessage(body);
    return message.request_id;
  }

  function receive(message) {
    switch (message.type) {
      case "render": render(message); break;
      case "scratchpad":
        if (!sameContext(message.context, state.doc?.context) || message.request_id !== scratchpad.request_id) break;
        Object.assign(scratchpad, {
          documents: message.documents, found: message.found, warnings: message.warnings,
          available: message.available, loading: false,
        });
        if (!pickerEl.hidden && picker.kind === "documents" && picker.scope === "scratchpad") updatePickerItems();
        break;
      case "bookmarks":
        if (!sameContext(message.context, state.doc?.context) || message.request_id !== bookmarks.request_id) break;
        bookmarks.documents = message.documents;
        bookmarks.found = message.found;
        bookmarks.warnings = message.warnings;
        bookmarks.loading = false;
        if (!pickerEl.hidden && picker.kind === "documents" && picker.scope === "bookmarks") updatePickerItems();
        break;
      case "documents":
        if (sameContext(message.context, state.doc?.context) && state.doc?.sessionId === message.session_id) {
          state.doc.documents = message.documents;
          if (state.lastRender) state.lastRender.documents = message.documents;
          renderDocuments();
          if (!pickerEl.hidden && picker.kind === "documents" && picker.scope === "current") updatePickerItems();
        }
        break;
      case "banner":
        bannerEl.textContent = message.text;
        bannerEl.hidden = false;
        break;
      case "toast": toast(message.text); break;
      case "live-sessions": dropEnded(message.sessions); break;
      case "theme": applyTheme(message.theme); break;
      case "send-result": onSendResult(message); break;
      default: console.warn("unknown message", message);
    }
  }

  // The daemon's entry point into the page; frozen so nothing rendered
  // from a document can replace it.
  Object.defineProperty(window, "peekback", {
    value: Object.freeze({ receive }),
    writable: false,
    configurable: false,
  });

  setMode("normal");
  post({ type: "ready" });
})();
