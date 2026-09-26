// Every built-in theme defines the same tokens and stays readable: 4.5:1 for
// text (WCAG 1.4.3) and 3:1 for the cursor and comment markers (1.4.11).
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");

const dir = path.join(__dirname, "../assets/themes");
const tokens = (css) => Object.fromEntries([...css.matchAll(/--([a-z0-9-]+):\s*([^;]+);/g)].map((m) => [m[1], m[2].trim()]));
const rgb = (hex) => [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16) / 255);
const luminance = (hex) => {
  const [r, g, b] = rgb(hex).map((v) => (v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4));
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
};
const contrast = (a, b) => {
  const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return (hi + 0.05) / (lo + 0.05);
};
// The selection tint, as app.css mixes it: 12% of the accent into the background.
const mix = (a, b, t) => "#" + rgb(a).map((v, i) => Math.round((v * t + rgb(b)[i] * (1 - t)) * 255).toString(16).padStart(2, "0")).join("");

const files = fs.readdirSync(dir).filter((f) => f.endsWith(".css")).sort();
const names = files.map((f) => f.replace(/\.css$/, ""));
const table = [...fs.readFileSync(path.join(__dirname, "../src/theme.rs"), "utf8").matchAll(/\("([a-z0-9-]+)", Appearance::/g)].map((m) => m[1]);
assert.deepEqual(names, [...table].sort(), "each stylesheet needs an entry in src/theme.rs THEMES, and each entry a stylesheet");

let expected = null;
for (const name of names) {
  const css = fs.readFileSync(path.join(dir, `${name}.css`), "utf8");
  assert.match(css, new RegExp(`:root\\[data-theme="${name}"\\]`), `${name}.css must scope its tokens to its name`);
  const t = tokens(css);
  const keys = Object.keys(t).sort();
  expected ??= keys;
  assert.deepEqual(keys, expected, `${name}.css must define the same tokens as ${names[0]}.css`);
  const selection = mix(t.accent, t.bg, 0.12);
  const pairs = [
    ["text", t.fg, t.bg, 4.5], ["text on panel", t.fg, t["panel-bg"], 4.5], ["text on code", t.fg, t["code-bg"], 4.5],
    ["text on selection", t.fg, selection, 4.5], ["text on comment", t.fg, t["annotate-bg"], 4.5],
    ["muted", t.muted, t.bg, 4.5], ["muted on panel", t.muted, t["panel-bg"], 4.5], ["muted on code", t.muted, t["code-bg"], 4.5],
    ["link", t.accent, t.bg, 4.5], ["current item", t.accent, t["panel-bg"], 4.5], ["button", t["on-accent"], t.accent, 4.5],
    ["banner", t["banner-fg"], t["banner-bg"], 4.5], ["comment badge", t.annotate, t["annotate-bg"], 4.5],
    ["normal mode", t.bg, t.muted, 4.5], ["visual mode", t.bg, t["mode-visual"], 4.5], ["active mode", t.bg, t.accent, 4.5],
    ["search match", t["search-fg"], t["search-bg"], 4.5],
    ["cursor", t.accent, t.bg, 3], ["comment marker", t.annotate, t.bg, 3], ["dimmed comment marker", t["annotate-dim"], t.bg, 3],
    ...Object.keys(t).filter((k) => k.startsWith("syn-")).map((k) => [k, t[k], t["code-bg"], 4.5]),
  ];
  for (const [what, fg, bg, minimum] of pairs) {
    assert.ok(/^#[0-9a-f]{6}$/i.test(fg) && /^#[0-9a-f]{6}$/i.test(bg), `${name}: ${what} must use #rrggbb colors`);
    const ratio = contrast(fg, bg);
    assert.ok(ratio >= minimum, `${name}: ${what} is ${ratio.toFixed(2)}:1, below ${minimum}:1`);
  }
}
console.log(`Themes passed: ${names.join(", ")} define the same tokens and meet WCAG contrast.`);
