import assert from "node:assert/strict";
import { test } from "node:test";

import { squarify } from "../dist/src/selectors/treemap.js";

const area = (rect) => rect.width * rect.height;
const close = (actual, expected, message) =>
  assert.ok(Math.abs(actual - expected) < 1e-6, `${message}: ${actual} vs ${expected}`);

test("the rectangles fill the box exactly and each is proportional to its value", () => {
  // The worked example from the squarified-treemap paper: 24 units in 6 by 4.
  const items = [
    { key: "a", value: 6 },
    { key: "b", value: 6 },
    { key: "c", value: 4 },
    { key: "d", value: 3 },
    { key: "e", value: 2 },
    { key: "f", value: 2 },
    { key: "g", value: 1 },
  ];
  const rects = squarify(items, 6, 4);
  assert.equal(rects.length, items.length);
  close(
    rects.reduce((total, rect) => total + area(rect), 0),
    24,
    "the areas add up to the box",
  );
  for (const item of items) {
    const rect = rects.find((entry) => entry.key === item.key);
    close(area(rect), item.value, `${item.key} has its share`);
    assert.ok(rect.x >= -1e-9 && rect.y >= -1e-9, `${item.key} starts inside the box`);
    assert.ok(rect.x + rect.width <= 6 + 1e-9, `${item.key} ends inside the box`);
    assert.ok(rect.y + rect.height <= 4 + 1e-9, `${item.key} ends inside the box`);
  }
  // The first row is the two sixes down the left, 3 wide and 2 tall each.
  assert.deepEqual(rects[0], { key: "a", x: 0, y: 0, width: 3, height: 2 });
  assert.deepEqual(rects[1], { key: "b", x: 0, y: 2, width: 3, height: 2 });
});

test("equal values in a square make squares", () => {
  const rects = squarify(
    [
      { key: "a", value: 5 },
      { key: "b", value: 5 },
      { key: "c", value: 5 },
      { key: "d", value: 5 },
    ],
    2,
    2,
  );
  for (const rect of rects) {
    close(rect.width, 1, `${rect.key} is one wide`);
    close(rect.height, 1, `${rect.key} is one tall`);
  }
});

test("items with no positive value take no room, and an empty box lays out nothing", () => {
  const rects = squarify(
    [
      { key: "a", value: 10 },
      { key: "zero", value: 0 },
      { key: "refund", value: -5 },
      { key: "nan", value: Number.NaN },
    ],
    10,
    10,
  );
  assert.deepEqual(rects, [{ key: "a", x: 0, y: 0, width: 10, height: 10 }]);
  assert.deepEqual(squarify([], 10, 10), []);
  assert.deepEqual(squarify([{ key: "a", value: 1 }], 0, 10), []);
});

test("many small items still tile the box without a sliver of a rectangle", () => {
  const items = Array.from({ length: 40 }, (unused, index) => ({
    key: `k${index}`,
    value: 100 - index * 2,
  }));
  const rects = squarify(items, 400, 300);
  close(
    rects.reduce((total, rect) => total + area(rect), 0),
    120_000,
    "the areas add up to the box",
  );
  const worst = Math.max(
    ...rects.map((rect) => Math.max(rect.width / rect.height, rect.height / rect.width)),
  );
  assert.ok(worst < 8, `the worst aspect ratio is ${worst.toFixed(2)}`);
});
