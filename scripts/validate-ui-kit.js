#!/usr/bin/env node
// Every web surface draws its controls from the design system.
//
// A raw `<button>`, `<input>`, `<select>`, `<textarea>`, or `<dialog>` in an
// application's source is a control the kit did not style, focus, or label:
// it looks different, and it is the one a keyboard user trips on. This walks
// the applications' sources and refuses any such element outside the kit,
// naming the file and line, so adoption is checked rather than hoped for.
//
// The kit itself (`packages/ui`) is where those elements are allowed to live.
// An application may keep a short allow-list of files with a reason -- an
// `<input type="file">` the kit has no wrapper for, say -- and a whole
// application can be allow-listed by path while it is still being moved over,
// which is what `apps/console` is until its re-skin lands.
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

export const RAW_CONTROL = /<(button|input|select|textarea|dialog)(?=[\s/>])/g;

/** Where applications live, relative to the repository root. */
export const APPLICATION_SOURCES = ["examples/rational/src", "apps/console/src"];

/**
 * Sources that may still render a raw control, each with the reason. A path
 * ending in `/` allow-lists everything under it.
 */
export const ALLOWED = new Map([
  ["apps/console/src/", "the console adopts the kit in a later task of ui-design-system"],
]);

/** Elements the kit deliberately leaves to the application. */
export const ALLOWED_RAW = [
  // A file picker has no styled equivalent worth having; the kit's Button labels it.
  /<input\s[^>]*type="file"/,
  /<input\s[^>]*type="hidden"/,
];

function* sourceFiles(directory) {
  for (const entry of readdirSync(directory)) {
    const path = join(directory, entry);
    if (statSync(path).isDirectory()) {
      yield* sourceFiles(path);
    } else if (/\.(tsx|jsx)$/.test(entry)) {
      yield path;
    }
  }
}

function isAllowed(relativePath) {
  for (const allowed of ALLOWED.keys()) {
    if (allowed.endsWith("/") ? relativePath.startsWith(allowed) : relativePath === allowed) {
      return true;
    }
  }
  return false;
}

/**
 * Every raw control in the applications under `root`, as
 * `{ file, line, element }`, skipping allow-listed files and forms.
 */
export function findRawControls(root, sources = APPLICATION_SOURCES) {
  const findings = [];
  for (const source of sources) {
    const directory = resolve(root, source);
    let files;
    try {
      files = [...sourceFiles(directory)];
    } catch {
      continue;
    }
    for (const file of files) {
      const relativePath = relative(root, file).split("\\").join("/");
      if (isAllowed(relativePath)) continue;
      const text = readFileSync(file, "utf8");
      const lines = text.split("\n");
      lines.forEach((line, index) => {
        for (const match of line.matchAll(RAW_CONTROL)) {
          const tag = line.slice(match.index);
          if (ALLOWED_RAW.some((pattern) => pattern.test(tag))) continue;
          findings.push({ file: relativePath, line: index + 1, element: match[1] });
        }
      });
    }
  }
  return findings;
}

export function main(root, log = console) {
  const findings = findRawControls(root);
  if (findings.length > 0) {
    for (const finding of findings) {
      log.error(
        `${finding.file}:${finding.line}: raw <${finding.element}> outside the design system; use the kit's component`,
      );
    }
    log.error(
      `${findings.length} raw control${findings.length === 1 ? "" : "s"} in application sources`,
    );
    return 1;
  }
  const applications = APPLICATION_SOURCES.filter((source) => !isAllowed(`${source}/`));
  log.log(
    `validated ${applications.length} application source tree${applications.length === 1 ? "" : "s"} against the design system; no raw controls`,
  );
  return 0;
}

if (process.argv[1] !== undefined && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const root = resolve(fileURLToPath(new URL("..", import.meta.url)));
  process.exit(main(root));
}
