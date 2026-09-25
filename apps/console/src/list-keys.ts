/**
 * Pairs each entry of a list with a React key built from the entry itself.
 * Observations can repeat exactly (the same sample twice in one second, a
 * health record with no region), so the second and later copies of the same
 * identity get a count; unlike a position in the list, the key of an entry
 * does not change when an entry before it is added or removed.
 */
export function withOccurrenceKeys<T>(
  items: readonly T[],
  identity: (item: T) => string,
): { readonly item: T; readonly key: string }[] {
  const seen = new Map<string, number>();
  return items.map((item) => {
    const base = identity(item);
    const count = seen.get(base) ?? 0;
    seen.set(base, count + 1);
    return { item, key: count === 0 ? base : `${base}#${count}` };
  });
}
