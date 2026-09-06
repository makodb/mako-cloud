/**
 * A squarified treemap layout (Bruls, Huizing, and van Wijk, 2000).
 *
 * The naive slice-and-dice layout gives every item a sliver as thin as its
 * share is small, and a sliver cannot carry a label. Squarifying lays items
 * out in rows along the shorter side of what is left, adding to a row while
 * doing so keeps the row's worst aspect ratio from getting worse, so the
 * rectangles come out as near to squares as the numbers allow. Pure
 * geometry, no DOM: the spending page hands it slices and draws what comes
 * back, and the test can check that the areas add up.
 */

export interface TreemapItem {
  readonly key: string;
  readonly value: number;
}

export interface TreemapRect {
  readonly key: string;
  readonly x: number;
  readonly y: number;
  readonly width: number;
  readonly height: number;
}

interface Scaled {
  readonly key: string;
  readonly area: number;
}

/** The worst aspect ratio in a row of the given total laid along a side of the given length. */
function worst(row: readonly Scaled[], sum: number, side: number): number {
  const sideSquared = side * side;
  const sumSquared = sum * sum;
  let ratio = 0;
  for (const item of row) {
    const candidate = Math.max(
      (sideSquared * item.area) / sumSquared,
      sumSquared / (sideSquared * item.area),
    );
    if (candidate > ratio) ratio = candidate;
  }
  return ratio;
}

/**
 * Rectangles for the positive items, largest first, filling `width` by
 * `height` exactly. Items with no positive value take no room and are left
 * out rather than drawn as nothing.
 */
export function squarify(
  items: readonly TreemapItem[],
  width: number,
  height: number,
): readonly TreemapRect[] {
  const positive = items
    .filter((item) => Number.isFinite(item.value) && item.value > 0)
    .sort((left, right) => right.value - left.value || left.key.localeCompare(right.key));
  const total = positive.reduce((sum, item) => sum + item.value, 0);
  if (total <= 0 || !(width > 0) || !(height > 0)) return [];
  const scale = (width * height) / total;
  let remaining: readonly Scaled[] = positive.map((item) => ({
    key: item.key,
    area: item.value * scale,
  }));

  const rects: TreemapRect[] = [];
  let x = 0;
  let y = 0;
  let free = { width, height };
  while (remaining.length > 0) {
    const first = remaining[0];
    if (first === undefined) break;
    const side = Math.min(free.width, free.height);
    if (side <= 0) {
      // Rounding has used up the box; whatever is left is too small to see.
      for (const item of remaining) rects.push({ key: item.key, x, y, width: 0, height: 0 });
      break;
    }
    const row: Scaled[] = [first];
    let sum = first.area;
    let ratio = worst(row, sum, side);
    let taken = 1;
    while (taken < remaining.length) {
      const next = remaining[taken];
      if (next === undefined) break;
      const candidate = worst([...row, next], sum + next.area, side);
      if (candidate > ratio) break;
      row.push(next);
      sum += next.area;
      ratio = candidate;
      taken += 1;
    }
    if (free.width >= free.height) {
      // A column down the left, as tall as the free box; the box narrows.
      const columnWidth = sum / free.height;
      let offset = y;
      for (const item of row) {
        const itemHeight = item.area / columnWidth;
        rects.push({ key: item.key, x, y: offset, width: columnWidth, height: itemHeight });
        offset += itemHeight;
      }
      x += columnWidth;
      free = { width: free.width - columnWidth, height: free.height };
    } else {
      // A row across the top, as wide as the free box; the box shortens.
      const rowHeight = sum / free.width;
      let offset = x;
      for (const item of row) {
        const itemWidth = item.area / rowHeight;
        rects.push({ key: item.key, x: offset, y, width: itemWidth, height: rowHeight });
        offset += itemWidth;
      }
      y += rowHeight;
      free = { width: free.width, height: free.height - rowHeight };
    }
    remaining = remaining.slice(taken);
  }
  return rects;
}
