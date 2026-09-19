// Shared by the validators that point at a section of one of the two books:
// a link such as `docs/dev-book.md#runbook-storage-contract-failure` must name
// a file that exists and a heading that exists in it, so a renamed chapter
// fails the build instead of leaving an alert or a gate pointing at nothing.

import { access, readFile } from "node:fs/promises";
import { resolve } from "node:path";

/** GitHub-style anchors for every heading, numbering duplicates as GitHub does. */
export function headingAnchors(markdown) {
  const texts = new Set();
  const anchors = new Set();
  const seen = new Map();
  let inFence = false;
  for (const line of markdown.split("\n")) {
    if (/^\s*```/.test(line)) inFence = !inFence;
    if (inFence) continue;
    const match = line.match(/^#{1,6}\s+(.+?)\s*$/);
    if (match === null) continue;
    const text = match[1].replace(/`/g, "");
    texts.add(text);
    const base = text
      .toLowerCase()
      .replace(/[^\p{L}\p{N}\s-]/gu, "")
      .trim()
      .replace(/\s+/g, "-");
    const count = seen.get(base) ?? 0;
    seen.set(base, count + 1);
    anchors.add(count === 0 ? base : `${base}-${count}`);
  }
  return { texts, anchors };
}

/**
 * Requires `link` (a repository-relative path, optionally with a `#fragment`)
 * to exist and, when it names a Markdown fragment, to name a heading in it.
 */
export async function requireMarkdownTarget(root, link, context) {
  const [path, fragment] = link.split("#", 2);
  await access(resolve(root, path)).catch(() => {
    throw new Error(`${context} links to a missing file: ${link}`);
  });
  if (fragment === undefined || !path.endsWith(".md")) return;
  const { anchors } = headingAnchors(await readFile(resolve(root, path), "utf8"));
  if (!anchors.has(fragment)) {
    throw new Error(`${context} links to a missing section: ${link}`);
  }
}
