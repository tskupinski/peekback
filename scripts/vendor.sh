#!/bin/bash
# Downloads the pinned front-end libraries into assets/vendor. Re-run after
# bumping a version below; the files are embedded in the binary at build time.
set -euo pipefail

MARKDOWN_IT=15.0.2
MD_FOOTNOTE=4.0.0
MD_TASK_LISTS=2.1.1
MERMAID=12.0.0
KATEX=0.18.7
HLJS=11.12.0

cdn="https://cdn.jsdelivr.net/npm"
out="$(cd "$(dirname "$0")/.." && pwd)/assets/vendor"
rm -rf "$out"
mkdir -p "$out/katex/fonts" "$out/hljs"

fetch() { curl -fsSL "$1" -o "$2"; echo "  $2"; }

fetch "$cdn/markdown-it@$MARKDOWN_IT/dist/browser/markdown-it.umd.min.js" "$out/markdown-it.min.js"
fetch "$cdn/markdown-it-footnote@$MD_FOOTNOTE/dist/markdown-it-footnote.min.js" "$out/markdown-it-footnote.min.js"
fetch "$cdn/markdown-it-task-lists@$MD_TASK_LISTS/dist/markdown-it-task-lists.min.js" "$out/markdown-it-task-lists.min.js"
fetch "$cdn/mermaid@$MERMAID/dist/mermaid.min.js" "$out/mermaid.min.js"
fetch "$cdn/katex@$KATEX/dist/katex.min.js" "$out/katex/katex.min.js"
fetch "$cdn/katex@$KATEX/dist/katex.min.css" "$out/katex/katex.min.css"
fetch "$cdn/katex@$KATEX/dist/contrib/auto-render.min.js" "$out/katex/auto-render.min.js"
fetch "$cdn/@highlightjs/cdn-assets@$HLJS/highlight.min.js" "$out/hljs/highlight.min.js"
fetch "$cdn/@highlightjs/cdn-assets@$HLJS/styles/github.min.css" "$out/hljs/github.min.css"
fetch "$cdn/@highlightjs/cdn-assets@$HLJS/styles/github-dark.min.css" "$out/hljs/github-dark.min.css"

# WebKit picks woff2 first, so the woff and ttf fallbacks are not needed.
curl -fsSL "https://data.jsdelivr.com/v1/package/npm/katex@$KATEX/flat" \
  | python3 -c 'import sys,json; [print(f["name"]) for f in json.load(sys.stdin)["files"] if f["name"].startswith("/dist/fonts/") and f["name"].endswith(".woff2")]' \
  | while read -r f; do fetch "$cdn/katex@$KATEX$f" "$out/katex/fonts/$(basename "$f")"; done

cat > "$out/VERSIONS" <<V
markdown-it $MARKDOWN_IT
markdown-it-footnote $MD_FOOTNOTE
markdown-it-task-lists $MD_TASK_LISTS
mermaid $MERMAID
katex $KATEX
highlight.js $HLJS
V
echo "vendored into $out"
