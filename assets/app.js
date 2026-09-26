(() => {
  const $ = (id) => document.getElementById(id);
  const docEl = $("doc");
  const bannerEl = $("banner");
  const noDocumentsEl = $("no-documents");
  const noDocumentsText = noDocumentsEl.textContent.trim();
  const composerEl = $("composer");
  const composerQuote = $("composer-quote");
  const composerInput = $("composer-input");
  const statusPendingEl = $("status-pending");
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
    comments: [], // { id, owner, path, fileIdentity, quote, note, line, endLine, anchorSource, anchorCount, anchorIndex, changed }
    question: null, // { text, yes, no, shown }: the one question, about a clipboard send
    pendingJump: null, // { fileIdentity, line }: where to go once that document renders
    undo: null, // { comment } removed, or { what, note } a discarded note: what u brings back
    currentComment: null, // the comment ]c and [c last stopped on
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
    // A note being written belongs to the document and session it was started on.
    const noteOn = composing && (composing.target ?? composing.editing);
    if (noteOn && (noteOn.fileIdentity !== state.doc.fileIdentity || noteOn.owner !== commentOwner())) settleComposer();
    // Another session's comments wait for it; coming back finds them.
    const restored = previousOwner !== commentOwner() ? activeComments().length : 0;
    // A search belongs to what you were reading in that session.
    if (previousOwner !== commentOwner() && state.search.query) clearSearch();
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
    // The note being written follows its text like a saved comment does.
    if (composing?.target?.anchorSource) locate(composing.target, message.source.split("\n"));
    renderComments();
    placeComposer();
    // A jump waiting for a document applies when that one renders; any
    // other render means it went elsewhere.
    // A changed selection cannot stay; a note being written on it stays open.
    if (sourceChanged && state.mode === "visual") setMode("normal");
    if (state.pendingJump?.fileIdentity === state.doc.fileIdentity) {
      state.cursor = nearestBlock(state.pendingJump.line);
    } else {
      state.cursor = previousLine === undefined ? 0 : nearestBlock(Number(previousLine));
    }
    state.pendingJump = null;
    if (state.search.query) runSearch(state.search.query, { keepIndex: true });
    paintCursor(false);
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
    if (state.question?.shown && mode !== "confirm") {
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

  // While a question is shown, it owns the status message.
  function setMessage(text) {
    if (state.question?.shown) return;
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
    statusDocEl.textContent = parts.join(" › ");
    // Pending comments stay in view, with the key that sends them.
    const pending = activeComments().length;
    const changed = activeComments().filter((c) => c.changed).length;
    statusPendingEl.hidden = pending === 0;
    statusPendingEl.textContent = `${pending} pending${changed ? ` · ${changed} changed` : ""} · S send`;
  }


  // --------------------------------------------------------------- actions

  function switchTo(sessionId, path) {
    if (state.question?.shown) return;
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
    if (rendering || state.question?.shown || !text) return;
    endLine = lastTextLine(line, endLine);
    if (action === "copy") post({ type: "copy", text });
    if (action === "send") {
      const quoted = [lineRef(state.doc?.fileIdentity, line, endLine), blockquote(text)].filter(Boolean).join("\n");
      const attached = commentsWithin(line, endLine);
      if (attached.length) sendComments(attached, `${quoted}\n\n`, { quotes: false });
      else post({ type: "send", text: quoted, purpose: "selection" });
    }
    if (action === "comment") openComposer({ target: commentTarget(text, line, endLine) });
  }

  // -------------------------------------------------------------- comments

  // Comments belong to their session and live in memory until they are sent
  // or discarded, their session ends, or the daemon stops. Moving to another
  // session parks them.
  let nextCommentId = 1;
  let commentsInFlight = null; // { request_id, sent } for a send that has not reported back
  // Tells the agent a quote is no longer in the file, so it is not read as a
  // request to restore it.
  const CHANGED_NOTE = "(This text has since changed or been removed.)";

  // markdown-it counts the blank line after a list as part of its last item;
  // a line reference and an anchor end where the text does.
  function lastTextLine(line, endLine) {
    if (!Number.isInteger(line) || !Number.isInteger(endLine)) return endLine;
    const lines = state.doc.source.split("\n");
    while (endLine > line + 1 && !lines[endLine - 1]?.trim()) endLine--;
    return endLine;
  }

  function commentTarget(quote, line, endLine) {
    if (!state.doc) return null;
    endLine = lastTextLine(line, endLine);
    return {
      path: state.doc.path,
      fileIdentity: state.doc.fileIdentity,
      owner: commentOwner(),
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
    composerInput.value = what.draft ?? note;
    composerQuote.textContent = firstLine(quote);
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

  // Leaving the note's document saves what was written, with its own
  // document and session, rather than carrying the box to another document.
  function settleComposer() {
    const what = resolveNote(composing);
    const note = composerInput.value.trim();
    closeComposer();
    if (!note) return;
    if (what.editing) what.editing.note = note;
    else state.comments.push({ ...what.target, id: nextCommentId++, note });
    toast("note saved with its document");
  }

  // An empty note is no comment: a new one is not added, and emptying an
  // existing one removes it (u brings it back).
  function saveComposer() {
    const what = resolveNote(composing);
    const note = composerInput.value.trim();
    closeComposer();
    if (what?.editing) {
      if (!note) return removeComment(what.editing.id);
      what.editing.note = note;
      renderComments();
      setMessage("comment updated");
    } else if (what?.target && note) {
      addComment(what.target, note);
    }
  }

  // The comment a note edits may have been sent or removed since; the note
  // then becomes a new comment on the same text rather than going nowhere.
  function resolveNote(what) {
    if (!what?.editing) return what;
    const editing = state.comments.find((c) => c.id === what.editing.id);
    if (editing) return { editing };
    const { id, note, changed, ...target } = what.editing;
    return { target };
  }

  // Esc on a note with writing in it keeps the writing for u.
  function discardComposer() {
    const what = composing;
    const note = composerInput.value.trim();
    closeComposer();
    const unchanged = what?.editing ? note === what.editing.note : !note;
    if (unchanged) return;
    state.undo = { what: { target: what.target, editing: what.editing }, note };
    toast("note discarded · u restores");
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

  // Bars follow the text when the layout changes: a resize, an image loading.
  if (typeof ResizeObserver === "function") {
    new ResizeObserver(() => { if (!rendering) { drawGutter(attachedHere()); placeComposer(); } }).observe(docEl);
  }

  // This document's comments, and those whose text is still where they say
  // (a comment from a mouse selection without source lines has no place).
  function commentsHere() {
    return activeComments().filter((c) => c.fileIdentity === state.doc?.fileIdentity);
  }

  function attachedHere() {
    return commentsHere().filter((c) => !c.changed && c.anchorSource !== null);
  }
  composerInput.addEventListener("keydown", (event) => {
    event.stopPropagation();
    if (event.isComposing) return;
    if (event.key === "Escape") {
      event.preventDefault();
      discardComposer();
    } else if (event.key === "Enter" && !event.shiftKey) {
      event.preventDefault();
      saveComposer();
    }
  });

  // C edits and x removes the comment on the cursor's block; with several,
  // the latest.
  function onCommentAtCursor(act) {
    const block = state.blocks[state.cursor];
    if (!block) return;
    const start = Number(block.dataset.sourceLine);
    const end = Number(block.dataset.sourceEnd);
    const here = attachedHere().filter((c) => c.line < end && c.endLine > start);
    if (here.length === 0) return setMessage("no comment on this block · c adds one");
    act(here.at(-1));
    if (here.length > 1) setMessage(`${here.length} comments here · Space c lists them`);
  }

  function addComment(target, note) {
    if (!target?.quote) return;
    state.comments.push({
      ...target,
      id: nextCommentId++,
      note: note.trim(),
    });
    renderComments();
    setMessage("comment added"); // The status line already counts them.
  }

  // Removing is one key away, so it can always be taken back once with u.
  function removeComment(id) {
    const comment = state.comments.find((c) => c.id === id);
    if (!comment) return;
    state.comments = state.comments.filter((c) => c.id !== id);
    state.undo = { comment };
    renderComments();
    toast("comment removed · u restores");
  }

  function undo() {
    const last = state.undo;
    const owner = last?.comment?.owner ?? last?.what?.target?.owner ?? last?.what?.editing?.owner;
    if (!last || owner !== commentOwner()) return setMessage("nothing to undo");
    state.undo = null;
    if (last.comment) {
      state.comments.push(last.comment);
      renderComments();
      setMessage("comment restored");
    } else {
      const what = resolveNote(last.what);
      const on = what.editing ?? what.target;
      if (on.fileIdentity !== state.doc?.fileIdentity) {
        state.undo = last;
        return setMessage(`that note is on ${promptPathFor(on.path)} · u there restores it`);
      }
      openComposer({ ...what, draft: last.note });
    }
  }

  // Relative to the session's cwd when under it, otherwise absolute, since
  // the agent will read this path.
  function promptPathFor(path) {
    const session = state.doc?.session;
    if (session && path.startsWith(session.cwd + "/")) return path.slice(session.cwd.length + 1);
    return path;
  }

  // Each comment: where it is, the quote, then the note after a blank line,
  // so Markdown does not read the note as part of the quote. A selection
  // sent with its comments already quotes their text, so they skip it.
  function commentsPrompt(comments, { quotes = true } = {}) {
    const byPath = new Map();
    for (const c of comments) {
      if (!byPath.has(c.fileIdentity)) byPath.set(c.fileIdentity, []);
      byPath.get(c.fileIdentity).push(c);
    }
    return [...byPath.entries()]
      .map(([path, group]) => {
        const label = promptPathFor(path);
        const body = group
          .map((c) => {
            const head = [c.changed ? null : lineRef(path, c.line, c.endLine), quotes ? blockquote(c.quote) : null,
              c.changed ? CHANGED_NOTE : null].filter(Boolean).join("\n");
            return c.note ? `${head}\n\n${c.note}` : head;
          })
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

  // Send all reviews first: the sheet shows exactly what will be pasted.
  function sendAllComments() {
    openReview(activeComments());
  }

  // Until "Pasted them?" is answered, those comments may already be in the
  // prompt; sending again could paste them twice.
  function sendComments(comments, prefix, format) {
    if (state.question) return setMessage("answer whether the last comments were pasted first");
    if (commentsInFlight) return setMessage("still sending the previous batch");
    const request_id = post({ type: "send", text: prefix + commentsPrompt(comments, format), purpose: "comments" });
    if (request_id !== null) commentsInFlight = { request_id, sent: comments.map(({ id, note }) => ({ id, note })) };
  }

  // -------------------------------------------------------- review sheet

  const reviewEl = $("review");
  const reviewTarget = $("review-target");
  const reviewIntroInput = $("review-intro");
  const reviewPreview = $("review-preview");
  let reviewing = null; // ids of the comments the sheet would send
  // The opening line you last sent with, kept while the daemon runs.
  let reviewIntro = "Address these review comments.";

  function openReview(comments) {
    if (comments.length === 0) return setMessage("no pending comments for this session");
    if (state.question) return setMessage("answer whether the last comments were pasted first");
    closePicker();
    closeCmdline();
    reviewing = new Set(comments.map((c) => c.id));
    reviewIntroInput.value = reviewIntro;
    reviewEl.hidden = false;
    setMode("review");
    showReviewText();
    reviewIntroInput.focus();
  }

  // The comments under review as they are now: one removed or edited while
  // the sheet is open is not sent as it was.
  function reviewedComments() {
    return reviewing ? activeComments().filter((c) => reviewing.has(c.id)) : [];
  }

  function reviewPrefix() {
    const intro = reviewIntroInput.value.trim();
    return intro ? `${intro}\n\n` : "";
  }

  function showReviewText() {
    if (!reviewing) return;
    const comments = reviewedComments();
    if (comments.length === 0) {
      closeReview();
      return setMessage("nothing left to send");
    }
    const n = comments.length;
    const session = state.doc?.session;
    reviewTarget.textContent = session
      ? `Send ${n} comment${n === 1 ? "" : "s"} to ${sessionName(session)}`
      : "No session to send to: this document is not part of one";
    reviewPreview.textContent = reviewPrefix() + commentsPrompt(comments);
  }

  function closeReview() {
    if (reviewEl.hidden) return;
    reviewEl.hidden = true;
    reviewing = null;
    reviewIntroInput.blur();
    setMode("normal");
    showQuestionLater();
  }

  function sendReview() {
    if (!state.doc?.session) return;
    const comments = reviewedComments();
    const prefix = reviewPrefix();
    reviewIntro = reviewIntroInput.value.trim();
    closeReview();
    sendComments(comments, prefix);
  }

  reviewIntroInput.addEventListener("input", showReviewText);
  reviewIntroInput.addEventListener("keydown", (event) => {
    event.stopPropagation();
    if (event.isComposing) return;
    if (event.key === "Escape") {
      event.preventDefault();
      closeReview();
    } else if (event.key === "Enter") {
      event.preventDefault();
      sendReview();
    } else if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      reviewPreview.scrollTop += event.key === "ArrowDown" ? 48 : -48;
    }
  });

  // Only comments still as they were sent: one edited meanwhile stays
  // pending with its new note, and one removed meanwhile cannot come back
  // with u to be sent again.
  function markSent(sent) {
    const isSent = (c) => sent.some((s) => s.id === c.id && s.note === c.note);
    state.comments = state.comments.filter((c) => !isSent(c));
    if (state.undo?.comment && isSent(state.undo.comment)) state.undo = null;
    renderComments();
  }

  function onSendResult(message) {
    toast(message.text);
    if (message.purpose !== "comments" || message.request_id !== commentsInFlight?.request_id) return;
    const { sent } = commentsInFlight;
    commentsInFlight = null;
    if (message.ok && message.outcome === "pasted") {
      markSent(sent);
    } else if (message.ok && message.outcome === "copied") {
      // Only you know whether it reached the prompt; keeping them unasked
      // would send them again next time.
      const n = sent.length;
      ask(`Copied ${n} comment${n === 1 ? "" : "s"}. Pasted? y marks sent · n keeps`, () => markSent(sent), () => {});
    }
  }

  // Comments whose text lies wholly inside a selection travel with it; one
  // reaching beyond it was about more than you chose to send.
  function commentsWithin(line, endLine) {
    if (!Number.isInteger(line) || !Number.isInteger(endLine)) return [];
    return attachedHere().filter((c) => c.line >= line && c.endLine <= endLine);
  }

  // A one-key question in the status line, y or n, where Escape means n.
  // There is one: after a clipboard fallback, whether the comments were
  // pasted. It never interrupts typing: while an input is open it waits until
  // the input closes, so a note being written is kept.
  function ask(text, yes, no) {
    state.question = { text, yes, no, shown: false };
    showQuestion();
  }

  function showQuestion() {
    const question = state.question;
    if (!question || question.shown || inputOpen()) return;
    question.shown = true;
    setMode("confirm");
    statusMsgEl.textContent = question.text;
  }

  // After the key that closed an input has done its work, so its own message
  // does not come first.
  function showQuestionLater() {
    Promise.resolve().then(showQuestion);
  }

  function answer(key) {
    const choice = { y: state.question.yes, n: state.question.no, Escape: state.question.no }[key];
    if (!choice) return;
    state.question = null;
    setMode("normal");
    setMessage("");
    choice();
  }

  // Inputs take the keyboard; normal-mode keys and questions wait for them.
  function inputOpen() {
    return ["command", "search", "comment", "picker", "review"].includes(state.mode);
  }

  function renderComments() {
    paintComments();
    showReviewText();
    renderStatus();
  }

  // Finds where each comment on this document is now, then draws its bar.
  function paintComments() {
    const lines = state.doc?.source.split("\n") ?? [];
    for (const c of commentsHere()) c.changed = c.anchorSource !== null && !locate(c, lines);
    drawGutter(attachedHere());
  }

  // One bar per comment beside the blocks it covers, laid out from their
  // rendered positions; overlapping comments take the next lane over.
  const gutter = document.createElement("div");
  gutter.className = "comment-gutter";
  gutter.setAttribute("aria-hidden", "true");
  let gutterShownIn = null; // the render the gutter was last attached to

  function drawGutter(comments) {
    gutter.replaceChildren();
    if (!comments.length) return;
    // A render replaces the document's contents, gutter included.
    if (gutterShownIn !== renderGeneration) {
      docEl.append(gutter);
      gutterShownIn = renderGeneration;
    }
    const base = docEl.getBoundingClientRect();
    const lanes = [];
    const bars = [];
    for (const c of [...comments].sort((a, b) => a.line - b.line || b.endLine - a.endLine)) {
      const blocks = state.blocks.filter((b) => Number(b.dataset.sourceLine) < c.endLine && Number(b.dataset.sourceEnd) > c.line);
      if (!blocks.length) continue;
      const top = blocks[0].getBoundingClientRect().top - base.top;
      const bottom = blocks.at(-1).getBoundingClientRect().bottom - base.top;
      let lane = lanes.findIndex((end) => end <= top);
      if (lane < 0) lane = lanes.push(0) - 1;
      lanes[lane] = bottom;
      const bar = document.createElement("div");
      bar.className = "comment-bar" + (c.id === state.currentComment ? " current" : "");
      bar.dataset.commentId = String(c.id);
      bar.title = c.note || "(no note)";
      bar.style.top = `${top}px`;
      bar.style.height = `${Math.max(bottom - top, 4)}px`;
      bar.style.left = `${GUTTER_LEFT + Math.min(lane, 2) * 4}px`;
      bars.push(bar);
    }
    gutter.replaceChildren(...bars);
  }

  // Between the block cursor, 18px left of the text, and the text; three lanes
  // fit before the 32px padding.
  const GUTTER_LEFT = 20;

  // Reads notes without the mouse: steps through this document's comments in
  // order and shows each one's note.
  function nextComment(direction) {
    const here = commentsHere().sort((a, b) => a.line - b.line || a.id - b.id);
    if (here.length === 0) return setMessage("no comments in this document");
    const at = Number(state.blocks[state.cursor]?.dataset.sourceLine ?? -1);
    const current = here.findIndex((c) => c.id === state.currentComment);
    let i = current >= 0 && here[current].line === at
      ? current + direction
      : direction > 0 ? here.findIndex((c) => c.line > at) : here.findLastIndex((c) => c.line < at);
    if (i < 0 || i >= here.length) i = direction > 0 ? 0 : here.length - 1;
    const c = here[i];
    state.currentComment = c.id;
    moveCursor(nearestBlock(c.line));
    paintComments();
    const stale = c.changed ? " (text changed or removed)" : "";
    setMessage(`comment ${i + 1} of ${here.length}${stale}: ${c.note || "(no note)"}`);
  }

  // Text still on its lines stays put, even when a copy appears elsewhere.
  // Otherwise it follows text that moved, as when the agent adds a paragraph
  // above it, but only when the text occurs as often as when you commented,
  // and then the same copy in order: with one copy gone, such as a repeated
  // heading, there is no telling which was yours. Anything else is reported
  // as changed rather than guessed at.
  function locate(c, lines) {
    const found = occurrences(lines, c.anchorSource);
    if (lines.slice(c.line, c.endLine).join("\n") !== c.anchorSource) {
      if (found.length === 0 || found.length !== c.anchorCount) return false;
      const at = found[c.anchorIndex];
      [c.line, c.endLine] = [at, at + (c.endLine - c.line)];
    }
    // Counted again, so a later move is judged against the file as it is now.
    [c.anchorCount, c.anchorIndex] = [found.length, found.indexOf(c.line)];
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
    if (c.fileIdentity !== state.doc?.fileIdentity && !(state.doc?.documents ?? []).some((d) => d.path === c.path)) {
      return setMessage(`${promptPathFor(c.path)} is not in this session's documents · Ctrl-P opens it`);
    }
    if (c.fileIdentity !== state.doc?.fileIdentity) {
      state.pendingJump = { fileIdentity: c.fileIdentity, line: c.line };
      switchTo(state.doc?.sessionId ?? null, c.path);
      return;
    }
    if (c.changed) return setMessage("that text has changed or been removed");
    if (c.line !== undefined && c.line !== null) moveCursor(nearestBlock(c.line));
  }

  function commentCandidates() {
    return activeComments().map((c) => ({
      label: `${c.quote.split("\n")[0].slice(0, 60)}${c.note ? "  ·  " + c.note : ""}`,
      meta: `${promptPathFor(c.path)}${c.changed ? " · text changed or removed" : ""}`,
      id: c.id,
      run: () => jumpToComment(c),
    }));
  }

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
  function ago(unixSeconds) {
    const s = Math.max(0, Math.floor(Date.now() / 1000) - unixSeconds);
    if (s < 60) return "just now";
    if (s < 3600) return `${Math.floor(s / 60)}m ago`;
    if (s < 86400) return `${Math.floor(s / 3600)}h ago`;
    return `${Math.floor(s / 86400)}d ago`;
  }

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

  // An fzf-style overlay: type to filter, Enter to open. The one list of
  // documents and of comments.
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
    pickerInput.placeholder = { documents: "open document", comments: "pending comments, Ctrl-E edits, Ctrl-S sends one, Ctrl-D removes" }[kind];
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
    if (state.question?.shown) {
      if (modifierKeys.has(event.key)) return;
      event.preventDefault();
      return answer(event.key);
    }
    if (rendering && event.key !== "Escape") return;
    // A click elsewhere took focus from the note or the review sheet; the
    // next key brings it back, and Esc still closes it.
    const openInput = { comment: composerInput, review: reviewIntroInput }[state.mode];
    if (openInput) {
      event.preventDefault();
      if (event.key !== "Escape") return openInput.focus();
      return state.mode === "comment" ? discardComposer() : closeReview();
    }
    if (inputOpen()) return;
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
      else if (key === "c") nextComment(direction);
      else handled = false;
    } else {
      switch (key) {
        case "j": moveCursor(state.cursor + takeCount()); break;
        case "k": moveCursor(state.cursor - takeCount()); break;
        case "d": halfPage(1); break;
        case "u": undo(); break;
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
        case "C": onCommentAtCursor((c) => openComposer({ editing: c })); break;
        case "x": onCommentAtCursor((c) => removeComment(c.id)); break;
        case "S": sendAllComments(); break;
        case "/": openCmdline("/"); break;
        case ":": openCmdline(":"); break;
        case "n": gotoMatch(1); break;
        case "N": gotoMatch(-1); break;
        case "?": toggleHelp(); break;
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
                if (!pickerEl.hidden && picker.kind === "documents" && picker.scope === "current") updatePickerItems();
        }
        break;
      case "banner":
        bannerEl.textContent = message.text;
        bannerEl.hidden = false;
        break;
      case "toast": toast(message.text); break;
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
