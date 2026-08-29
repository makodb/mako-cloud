import type { Split } from "../model/types.js";

export type SplitValidation =
  | { readonly ok: true; readonly sum: number }
  | {
      readonly ok: false;
      readonly sum: number;
      /** What the splits are short by (positive) or over by (negative). */
      readonly difference: number;
      readonly reason: "sum_mismatch" | "invalid_amount";
    };

/**
 * Splits are optional; when present their amounts must add up to the
 * transaction amount exactly. The difference is reported so the editor can
 * show what is missing rather than merely refusing.
 */
export function validateSplits(amount: number, splits: readonly Split[]): SplitValidation {
  if (splits.length === 0) {
    return { ok: true, sum: 0 };
  }
  if (splits.some((split) => !Number.isSafeInteger(split.amount))) {
    const sum = splits.reduce(
      (total, split) => (Number.isSafeInteger(split.amount) ? total + split.amount : total),
      0,
    );
    return { ok: false, sum, difference: amount - sum, reason: "invalid_amount" };
  }
  const sum = splits.reduce((total, split) => total + split.amount, 0);
  if (sum !== amount) {
    return { ok: false, sum, difference: amount - sum, reason: "sum_mismatch" };
  }
  return { ok: true, sum };
}
