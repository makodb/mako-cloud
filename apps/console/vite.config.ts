import { mkdir, readFile, writeFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import tailwindcss from "@tailwindcss/vite";
import { marked } from "marked";
import { defineConfig, type Plugin } from "vite";

const USER_BOOK_SOURCE = resolve(import.meta.dirname, "../../docs/user-book.md");
const USER_BOOK_OUTPUT = resolve(import.meta.dirname, "web-dist/docs/user-book/index.html");

export default defineConfig({
  plugins: [tailwindcss(), userBookPlugin()],
  // The design system is a linked workspace package, so its dependencies are
  // not found by the dev server's first scan; naming them here has them
  // pre-bundled up front instead of on the first request.
  optimizeDeps: {
    include: [
      "react",
      "react-dom",
      "react-dom/client",
      "recharts",
      "radix-ui",
      "lucide-react",
      "clsx",
      "tailwind-merge",
      "class-variance-authority",
    ],
  },
  server: {
    // Suites running side by side write their artefacts next to the sources;
    // a dev server must not reload every page each time one of them does.
    watch: { ignored: ["**/test-results*/**", "**/playwright-report*/**"] },
  },
  build: {
    outDir: "web-dist",
    emptyOutDir: true,
    // The libraries change on their own schedule; kept apart from the
    // console's own code so a release does not invalidate every byte.
    rollupOptions: {
      output: {
        manualChunks: (id) => {
          if (!id.includes("node_modules")) return undefined;
          if (/[\\/]node_modules[\\/](react|react-dom|scheduler)[\\/]/.test(id)) return "react";
          if (
            /[\\/]node_modules[\\/](radix-ui|@radix-ui|@floating-ui|aria-hidden|react-remove-scroll)/.test(
              id,
            )
          )
            return "primitives";
          return "vendor";
        },
      },
    },
  },
});

/** Publish the canonical User Book with the console instead of maintaining a second docs copy. */
function userBookPlugin(): Plugin {
  const render = async () => userBookHtml(await readFile(USER_BOOK_SOURCE, "utf8"));
  const serve = (server: Parameters<NonNullable<Plugin["configureServer"]>>[0]) => {
    server.middlewares.use("/docs/user-book", async (_request, response, next) => {
      try {
        response.statusCode = 200;
        response.setHeader("Content-Type", "text/html; charset=utf-8");
        response.setHeader("Cache-Control", "no-cache");
        response.end(await render());
      } catch (error) {
        next(error as Error);
      }
    });
  };
  return {
    name: "mako-user-book",
    configureServer: serve,
    configurePreviewServer: serve,
    async closeBundle() {
      await mkdir(dirname(USER_BOOK_OUTPUT), { recursive: true });
      await writeFile(USER_BOOK_OUTPUT, await render(), "utf8");
    },
  };
}

function userBookHtml(markdown: string): string {
  const occurrences = new Map<string, number>();
  // References to contributor-only files make sense in the repository. The
  // hosted User Book keeps their labels without publishing dead links.
  const publicMarkdown = markdown.replace(
    /\[([^\]]+)\]\((?:\.\.\/[^)]+|dev-book\.md(?:#[^)]+)?)\)/gu,
    "$1",
  );
  // The source keeps a conventional contents list for repository readers. On
  // the hosted page those chapters live in the persistent docs navigation.
  const articleMarkdown = publicMarkdown.replace(
    /\n## Table of contents\n[\s\S]*?\n---\n/u,
    "\n---\n",
  );
  const body = String(marked.parse(articleMarkdown, { async: false })).replace(
    /<h([1-6])>(.*?)<\/h\1>/gu,
    (_heading, level: string, content: string) => {
      const base = headingId(content);
      const occurrence = occurrences.get(base) ?? 0;
      occurrences.set(base, occurrence + 1);
      const id = occurrence === 0 ? base : `${base}-${occurrence}`;
      return `<h${level} id="${id}">${content}<a class="heading-link" href="#${id}" aria-label="Link to this section">#</a></h${level}>`;
    },
  );
  const contents = userBookContents(body);
  return `<!doctype html>
<html lang="en">
  <head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <meta name="color-scheme" content="light dark">
    <meta name="description" content="The Mako Cloud User Book for application developers.">
    <title>Mako Cloud User Book</title>
    <style>${USER_BOOK_CSS}</style>
  </head>
  <body>
    <header class="site-header">
      <a class="brand" href="/" aria-label="Mako Cloud home"><span class="mark">◆</span>Mako Cloud</a>
      <nav class="site-nav" aria-label="Documentation"><a href="#table-of-contents">Contents</a><a href="/login">Console</a></nav>
    </header>
    <div class="docs-shell" id="table-of-contents">
      <aside class="desktop-toc" aria-label="Table of contents">
        <div class="toc-panel">
          <p class="toc-eyebrow">User Book</p>
          <p class="toc-title">On this page</p>
          <nav class="toc-nav">${contents}</nav>
        </div>
      </aside>
      <details class="mobile-toc">
        <summary><span>On this page</span><span aria-hidden="true">⌄</span></summary>
        <nav class="toc-nav" aria-label="Table of contents">${contents}</nav>
      </details>
      <main class="document">${body}</main>
    </div>
    <footer><a href="/">Mako Cloud</a><span>Cloud user documentation</span></footer>
    <script>${USER_BOOK_SCRIPT}</script>
  </body>
</html>`;
}

function userBookContents(body: string): string {
  return Array.from(body.matchAll(/<h2 id="([^"]+)">(.*?)<a class="heading-link"/gu))
    .filter(([, id]) => id !== "how-to-read-this-book")
    .map(([, id, content]) => {
      const label = content.replace(/<[^>]+>/gu, "");
      return `<a class="toc-link" href="#${id}" data-section="${id}">${label}</a>`;
    })
    .join("");
}

function headingId(content: string): string {
  return content
    .replace(/<[^>]+>/gu, "")
    .replaceAll("&amp;", "and")
    .replaceAll("&quot;", "")
    .toLowerCase()
    .replace(/[^a-z0-9\s-]/gu, "")
    .trim()
    .replace(/[\s-]+/gu, "-");
}

const USER_BOOK_CSS = `
:root { color-scheme: light dark; --bg: light-dark(#f8fafc,#191b21); --paper: light-dark(#fff,#20232a); --text: light-dark(#1c2029,#edf0f5); --muted: light-dark(#596273,#abb3c2); --line: light-dark(#e3e7ee,#353a45); --brand: light-dark(#365fd9,#91a9ff); --brand-soft: light-dark(#eaf0ff,#292f43); --code: light-dark(#f1f4f9,#17191e); }
* { box-sizing: border-box; }
html { scroll-behavior: smooth; scroll-padding-top: 5.5rem; background: var(--bg); color: var(--text); font: 16px/1.7 Inter, ui-sans-serif, system-ui, sans-serif; }
body { margin: 0; }
.site-header { position: sticky; top: 0; z-index: 2; display: flex; align-items: center; justify-content: space-between; min-height: 4rem; padding: 0.7rem max(1.25rem,calc((100% - 82rem)/2)); border-bottom: 1px solid var(--line); background: color-mix(in srgb,var(--bg) 88%,transparent); backdrop-filter: blur(16px); }
.brand { display: flex; align-items: center; gap: 0.65rem; color: var(--text); font-weight: 700; text-decoration: none; }
.mark { display: grid; width: 2rem; height: 2rem; place-items: center; border-radius: 0.6rem; background: var(--brand); color: var(--paper); font-size: 0.65rem; }
.site-nav { display: flex; gap: 1.25rem; font-size: 0.875rem; }
.docs-shell { display: grid; grid-template-columns: 15rem minmax(0,58rem); gap: 4rem; width: min(100% - 2.5rem,82rem); margin: 0 auto; align-items: start; }
.document { min-width: 0; padding: 4rem 0 6rem; }
.desktop-toc { position: sticky; top: 5rem; height: calc(100vh - 6.5rem); padding-top: 2.75rem; }
.toc-panel { height: 100%; overflow-y: auto; padding: 0 0.75rem 2rem 0; scrollbar-width: thin; scrollbar-color: var(--line) transparent; }
.toc-eyebrow { margin: 0 0 0.2rem; color: var(--brand); font-size: 0.7rem; font-weight: 700; letter-spacing: 0.14em; text-transform: uppercase; }
.toc-title { margin: 0 0 1rem; color: var(--text); font-size: 0.95rem; font-weight: 700; }
.toc-nav { display: grid; border-left: 1px solid var(--line); }
.toc-link { margin-left: -1px; padding: 0.42rem 0.65rem 0.42rem 0.9rem; border-left: 2px solid transparent; color: var(--muted); font-size: 0.8rem; line-height: 1.35; text-decoration: none; transition: border-color 120ms ease,background 120ms ease,color 120ms ease; }
.toc-link:hover { color: var(--text); }
.toc-link[aria-current="location"] { border-left-color: var(--brand); background: linear-gradient(90deg,var(--brand-soft),transparent); color: var(--text); font-weight: 600; }
.mobile-toc { display: none; }
h1,h2,h3,h4 { line-height: 1.2; letter-spacing: -0.025em; scroll-margin-top: 5.5rem; }
h1 { max-width: 46rem; margin: 0 0 1.5rem; font-size: clamp(2.5rem,7vw,4.5rem); }
h2 { margin: 4.5rem 0 1.2rem; padding-top: 1rem; border-top: 1px solid var(--line); font-size: clamp(1.8rem,4vw,2.5rem); }
h3 { margin: 2.75rem 0 0.8rem; font-size: 1.35rem; }
h4 { margin-top: 2rem; font-size: 1rem; }
.heading-link { margin-left: 0.5rem; color: var(--muted); font-weight: 400; text-decoration: none; opacity: 0; }
h1:hover .heading-link,h2:hover .heading-link,h3:hover .heading-link,h4:hover .heading-link,.heading-link:focus { opacity: 1; }
p,li { color: var(--muted); }
strong { color: var(--text); }
a { color: var(--brand); text-underline-offset: 0.2em; }
pre { overflow-x: auto; margin: 1.25rem 0; padding: 1rem 1.1rem; border: 1px solid var(--line); border-radius: 0.75rem; background: var(--code); font: 0.86rem/1.65 ui-monospace,SFMono-Regular,Menlo,monospace; }
code { overflow-wrap: anywhere; border-radius: 0.3rem; background: var(--code); padding: 0.15em 0.35em; color: var(--text); font: 0.88em ui-monospace,SFMono-Regular,Menlo,monospace; }
pre code { padding: 0; background: transparent; overflow-wrap: normal; word-break: normal; }
table { display: block; width: 100%; overflow-x: auto; margin: 1.5rem 0; border-collapse: collapse; font-size: 0.9rem; }
th,td { min-width: 9rem; padding: 0.7rem 0.8rem; border: 1px solid var(--line); text-align: left; vertical-align: top; }
th { background: var(--code); }
blockquote { margin: 1.5rem 0; padding: 0.2rem 1rem; border-left: 3px solid var(--brand); }
hr { margin: 4rem 0; border: 0; border-top: 1px solid var(--line); }
footer { display: flex; justify-content: space-between; gap: 1rem; padding: 2rem max(1.25rem,calc((100% - 82rem)/2)); border-top: 1px solid var(--line); color: var(--muted); font-size: 0.85rem; }
@media (max-width: 70rem) { .docs-shell { display: block; width: min(100% - 2.5rem,58rem); } .desktop-toc { display: none; } .mobile-toc { display: block; margin-top: 2rem; border: 1px solid var(--line); border-radius: 0.75rem; background: var(--paper); } .mobile-toc summary { display: flex; cursor: pointer; list-style: none; align-items: center; justify-content: space-between; padding: 0.8rem 1rem; color: var(--text); font-size: 0.9rem; font-weight: 650; } .mobile-toc summary::-webkit-details-marker { display: none; } .mobile-toc summary span:last-child { transition: transform 120ms ease; } .mobile-toc[open] summary { border-bottom: 1px solid var(--line); } .mobile-toc[open] summary span:last-child { transform: rotate(180deg); } .mobile-toc .toc-nav { max-height: 19rem; overflow-y: auto; margin: 0.65rem; border-left: 0; } .mobile-toc .toc-link { margin: 0; border-left: 2px solid transparent; border-radius: 0.35rem; } .document { padding-top: 2.75rem; } }
@media (max-width: 38rem) { .docs-shell { width: min(100% - 2rem,58rem); } .site-header { padding-inline: 1rem; } .site-nav a:first-child { display: none; } h2 { margin-top: 3.25rem; } footer { flex-direction: column; } }
@media (prefers-reduced-motion: reduce) { html { scroll-behavior: auto; } }
`;

const USER_BOOK_SCRIPT = `
(() => {
  const links = Array.from(document.querySelectorAll('.toc-link'));
  const headings = links
    .map((link) => document.getElementById(link.dataset.section))
    .filter(Boolean);
  const setCurrent = (id) => {
    for (const link of links) {
      if (link.dataset.section === id) link.setAttribute('aria-current', 'location');
      else link.removeAttribute('aria-current');
    }
  };
  const initial = location.hash.slice(1) || headings[0]?.id;
  if (initial) setCurrent(initial);
  const observer = new IntersectionObserver((entries) => {
    const visible = entries.find((entry) => entry.isIntersecting);
    if (visible) setCurrent(visible.target.id);
  }, { rootMargin: '-18% 0px -72% 0px' });
  for (const heading of headings) observer.observe(heading);
  for (const link of links) link.addEventListener('click', () => setCurrent(link.dataset.section));
})();
`;
