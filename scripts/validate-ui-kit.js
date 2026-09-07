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
// application can be allow-listed by path while it is still being moved over.
// Today nothing is excused.
//
// Only code is read: a comment explaining why a control was replaced, or a
// string carrying an example, names these elements without rendering one, and
// a validator that could not tell the difference would be a validator people
// learn to argue with.
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

export const RAW_CONTROL = /<(button|input|select|textarea|dialog)(?=[\s/>])/g;

/**
 * The source with its comments and string literals blanked out, one space per
 * character so every line and column still lines up with the file on disk.
 */
export function codeOnly(text) {
  const out = [...text];
  const blank = (start, end) => {
    for (let index = start; index < end && index < out.length; index += 1) {
      if (out[index] !== "\n") out[index] = " ";
    }
  };
  let index = 0;
  while (index < text.length) {
    const two = text.slice(index, index + 2);
    if (two === "//") {
      const end = text.indexOf("\n", index);
      blank(index, end === -1 ? text.length : end);
      index = end === -1 ? text.length : end;
    } else if (two === "/*") {
      const end = text.indexOf("*/", index + 2);
      blank(index, end === -1 ? text.length : end + 2);
      index = end === -1 ? text.length : end + 2;
    } else if (text[index] === '"' || text[index] === "'" || text[index] === "`") {
      const quote = text[index];
      let cursor = index + 1;
      while (cursor < text.length && text[cursor] !== quote) {
        cursor += text[cursor] === "\\" ? 2 : 1;
      }
      blank(index + 1, cursor);
      index = cursor + 1;
    } else {
      index += 1;
    }
  }
  return out.join("");
}

/** Where applications live, relative to the repository root. */
export const APPLICATION_SOURCES = ["examples/rational/src", "apps/console/src"];

/**
 * Sources that may still render a raw control, each with the reason. A path
 * ending in `/` allow-lists everything under it. Empty is the goal: every
 * surface draws its controls from the kit.
 */
export const ALLOWED = new Map();

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
      const source = readFileSync(file, "utf8");
      // Found in the code, but judged against what the line actually says: the
      // attribute that excuses a file picker lives inside a string, which the
      // blanked copy no longer carries.
      const written = source.split("\n");
      codeOnly(source)
        .split("\n")
        .forEach((line, index) => {
          for (const match of line.matchAll(RAW_CONTROL)) {
            const tag = (written[index] ?? "").slice(match.index);
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
