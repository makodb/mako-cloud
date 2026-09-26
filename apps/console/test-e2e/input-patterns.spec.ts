import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";

import { expect, test } from "@playwright/test";

// Browsers compile an input's pattern attribute with the regex `v` flag,
// where a literal "-" inside a character class must be escaped. An invalid
// pattern is only logged to the console and then ignored, so the field goes
// unchecked; fifteen ID fields were once in that state.
const SOURCE = join(import.meta.dirname, "..", "src");

function patterns(): { file: string; pattern: string }[] {
  const found: { file: string; pattern: string }[] = [];
  for (const file of readdirSync(SOURCE).filter((name) => name.endsWith(".tsx"))) {
    const text = readFileSync(join(SOURCE, file), "utf8");
    for (const match of text.matchAll(/pattern="([^"]+)"/gu)) {
      found.push({ file, pattern: match[1] ?? "" });
    }
    for (const match of text.matchAll(/_PATTERN = ("[^"]+");/gu)) {
      found.push({ file, pattern: JSON.parse(match[1] ?? '""') as string });
    }
  }
  return found;
}

test("every input pattern in the console is valid as browsers compile it", () => {
  const all = patterns();
  expect(all.length).toBeGreaterThan(10);
  const invalid = all.filter(({ pattern }) => {
    try {
      new RegExp(`^(?:${pattern})$`, "v");
      return false;
    } catch {
      return true;
    }
  });
  expect(invalid).toEqual([]);
});
