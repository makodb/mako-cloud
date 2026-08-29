import type { RxConflictHandler, WithDeleted } from "rxdb";

/**
 * Deterministic last-writer-wins. Both states of a conflict carry the same
 * primary key, so `updated_at` decides and, when two writers stamped the same
 * millisecond, the canonical JSON of the states breaks the tie — an ordering
 * every device computes identically, which is what makes the outcome the same
 * everywhere. Wall clocks are good enough for a household's edits; a server-
 * issued version would be the choice for anything adversarial.
 */
export interface Versioned {
  readonly id: string;
  readonly updated_at: number;
}

export function canonicalJson(value: unknown): string {
  if (Array.isArray(value)) {
    return `[${value.map(canonicalJson).join(",")}]`;
  }
  if (value !== null && typeof value === "object") {
    return `{${Object.entries(value as Record<string, unknown>)
      .filter(([, child]) => child !== undefined)
      .sort(([left], [right]) => (left < right ? -1 : left > right ? 1 : 0))
      .map(([key, child]) => `${JSON.stringify(key)}:${canonicalJson(child)}`)
      .join(",")}}`;
  }
  return JSON.stringify(value) ?? "null";
}

/** Replication metadata never takes part in equality or ordering. */
export function replicatedState<T extends object>(document: WithDeleted<T>): WithDeleted<T> {
  const copy: Record<string, unknown> = { ...document };
  delete copy._rev;
  delete copy._meta;
  delete copy._attachments;
  return copy as WithDeleted<T>;
}

export function sameState<T extends object>(left: WithDeleted<T>, right: WithDeleted<T>): boolean {
  return canonicalJson(replicatedState(left)) === canonicalJson(replicatedState(right));
}

/** Positive when `left` should win over `right`. */
export function compareStates<T extends Versioned>(
  left: WithDeleted<T>,
  right: WithDeleted<T>,
): number {
  if (left.updated_at !== right.updated_at) {
    return left.updated_at - right.updated_at;
  }
  const leftJson = canonicalJson(replicatedState(left));
  const rightJson = canonicalJson(replicatedState(right));
  return leftJson < rightJson ? -1 : leftJson > rightJson ? 1 : 0;
}

export function resolveConflict<T extends Versioned>(
  local: WithDeleted<T>,
  master: WithDeleted<T>,
): WithDeleted<T> {
  return compareStates(local, master) > 0 ? local : master;
}

export function conflictHandler<T extends Versioned>(): RxConflictHandler<T> {
  return {
    isEqual: (left, right) => sameState(left, right),
    resolve: async ({ newDocumentState, realMasterState }) =>
      resolveConflict(newDocumentState, realMasterState),
  };
}
