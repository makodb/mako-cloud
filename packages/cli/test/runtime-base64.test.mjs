import assert from "node:assert/strict";
import test from "node:test";
import { decodeBase64 } from "../runtime/main/supervisor.ts";

const maximum = 10 * 1024 * 1024;
test("runtime decodes valid deployment bundles up to its ten MiB bound", () => {
  for (const size of [0, 1, 2, 3, 4 * 1024 * 1024, maximum - 2, maximum - 1, maximum]) {
    const original = Buffer.alloc(size, 0xa7);
    const decoded = decodeBase64(original.toString("base64"), maximum);
    assert.equal(decoded.length, size);
    assert.ok(original.equals(decoded));
  }
});

test("runtime refuses malformed alphabet, grouping, padding and decoded overflow", () => {
  for (const value of ["A", "AA", "AAA", "A===", "====", "=AAA", "AA=A", "AA==AAAA", "YQ==\n", "Y Q=", "____", "éAAA"]) {
    assert.throws(() => decodeBase64(value, maximum), /invalid base64/u, value);
  }
  assert.throws(() => decodeBase64(Buffer.alloc(5).toString("base64"), 4), /decoded value exceeds limit/u);
  assert.throws(() => decodeBase64(Buffer.alloc(20).toString("base64"), 4), /invalid base64/u);
});
