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
// The sources are parsed, not scanned. A comment explaining why a control was
// replaced and a string carrying an example both name these elements without
// rendering one, and prose contains apostrophes; a reader working by regular
// expression either cries wolf or -- as an earlier version of this file did,
// blanking everything from the apostrophe in "Don't" to the next one -- goes
// quietly blind over the rest of the file.
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import ts from "typescript";

/** The elements an application must not render itself. */
export const RAW_CONTROLS = new Set(["button", "input", "select", "textarea", "dialog"]);

/** Where applications live, relative to the repository root. */
export const APPLICATION_SOURCES = ["examples/rational/src", "apps/console/src"];

/**
 * Sources that may still render a raw control, each with the reason. A path
 * ending in `/` allow-lists everything under it. Empty is the goal: every
 * surface draws its controls from the kit.
 */
export const ALLOWED = new Map();

/**
 * The `input` types the kit deliberately leaves to the application: a file
 * picker has no styled equivalent worth having, and a hidden input renders
 * nothing at all.
 */
export const ALLOWED_INPUT_TYPES = new Set(["file", "hidden"]);

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

/** The literal value of a JSX attribute; `null` when absent or computed. */
function literalAttribute(element, name) {
  for (const attribute of element.attributes.properties) {
    if (!ts.isJsxAttribute(attribute)) continue;
    if (attribute.name.getText() !== name) continue;
    const value = attribute.initializer;
    if (value === undefined) return "";
    if (ts.isStringLiteral(value)) return value.text;
    if (ts.isJsxExpression(value) && value.expression !== undefined) {
      if (ts.isStringLiteral(value.expression)) return value.expression.text;
      if (ts.isNoSubstitutionTemplateLiteral(value.expression)) return value.expression.text;
    }
    return null;
  }
  return null;
}

/** Every raw control the file renders, as `{ line, element }`, in source order. */
export function rawControlsIn(fileName, source) {
  const parsed = ts.createSourceFile(
    fileName,
    source,
    ts.ScriptTarget.Latest,
    true,
    ts.ScriptKind.TSX,
  );
  const found = [];
  const visit = (node) => {
    const opening = ts.isJsxSelfClosingElement(node)
      ? node
      : ts.isJsxElement(node)
        ? node.openingElement
        : null;
    // A capitalised tag is a component; only the intrinsic elements matter.
    if (opening !== null && ts.isIdentifier(opening.tagName)) {
      const element = opening.tagName.text;
      if (RAW_CONTROLS.has(element)) {
        const excused =
          element === "input" && ALLOWED_INPUT_TYPES.has(literalAttribute(opening, "type") ?? "");
        if (!excused) {
          const { line } = parsed.getLineAndCharacterOfPosition(opening.getStart(parsed));
          found.push({ line: line + 1, element });
        }
      }
    }
    ts.forEachChild(node, visit);
  };
  visit(parsed);
  return found;
}

/**
 * Every raw control in the applications under `root`, as
 * `{ file, line, element }`, skipping allow-listed files.
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
      for (const finding of rawControlsIn(file, readFileSync(file, "utf8"))) {
        findings.push({ file: relativePath, ...finding });
      }
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
