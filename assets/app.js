(() => {
  const docEl = document.getElementById("doc");
  const bannerEl = document.getElementById("banner");
  const sessionsEl = document.getElementById("sessions");
  const documentsEl = document.getElementById("documents");
  const toastEl = document.getElementById("toast");

  const darkMode = () => window.matchMedia("(prefers-color-scheme: dark)").matches;

  const md = window
    .markdownit({
      html: true,
      linkify: true,
      typographer: false,
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
        }
      }
    });
  }

  mermaid.initialize({ startOnLoad: false, theme: darkMode() ? "dark" : "default" });

  let current = null;
  let sessions = { sessions: [], current: null };

  async function render(path, source) {
    const scrollY = window.scrollY;
    current = { path, source };
    document.title = `${path.split("/").pop()} - peekback`;
    bannerEl.hidden = true;

    docEl.innerHTML = md.render(source);
    await renderMermaid();
    renderMathInElement(docEl, {
      delimiters: [
        { left: "$$", right: "$$", display: true },
        { left: "$", right: "$", display: false },
      ],
      throwOnError: false,
    });
    window.scrollTo(0, scrollY);
  }

  async function renderMermaid() {
    const blocks = docEl.querySelectorAll("pre > code.language-mermaid");
    const nodes = [];
    for (const code of blocks) {
      const pre = code.parentElement;
      const container = document.createElement("pre");
      container.className = "mermaid";
      container.textContent = code.textContent;
      if (pre.dataset.sourceLine) container.dataset.sourceLine = pre.dataset.sourceLine;
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
    if (current) render(current.path, current.source);
  });

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
    sessionsEl.replaceChildren(
      ...sessions.sessions.map((s) =>
        item({
          name: s.cwd.split("/").pop() || s.cwd,
          meta: ago(s.last_active_at),
          title: `${s.cwd}\n${s.session_id}`,
          current: s.session_id === sessions.current,
          onClick: () => send({ type: "switch", session_id: s.session_id }),
        })
      )
    );
    if (sessions.sessions.length === 0) {
      const li = document.createElement("li");
      li.className = "empty";
      li.textContent = "No live sessions";
      sessionsEl.append(li);
    }
  }

  function renderDocuments(documents, sessionId, currentPath) {
    documentsEl.replaceChildren(
      ...documents.map((d) => {
        const slash = d.label.lastIndexOf("/");
        return item({
          name: slash >= 0 ? d.label.slice(slash + 1) : d.label,
          meta: slash >= 0 ? d.label.slice(0, slash) : "",
          title: d.path,
          current: d.path === currentPath,
          onClick: () => send({ type: "switch", session_id: sessionId, path: d.path }),
        });
      })
    );
  }

  let toastTimer = null;
  function toast(text) {
    toastEl.textContent = text;
    toastEl.hidden = false;
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => (toastEl.hidden = true), 5000);
  }

  setInterval(renderSessions, 30000);

  window.peekback = {
    receive(message) {
      switch (message.type) {
        case "render":
          render(message.path, message.source);
          renderDocuments(message.documents, message.session?.session_id ?? null, message.path);
          break;
        case "sessions":
          sessions = message;
          renderSessions();
          break;
        case "banner":
          bannerEl.textContent = message.text;
          bannerEl.hidden = false;
          break;
        case "toast":
          toast(message.text);
          break;
        default:
          console.warn("unknown message", message);
      }
    },
  };

  send({ type: "ready" });
})();
