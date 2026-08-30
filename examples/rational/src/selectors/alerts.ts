import type { AlertKind } from "../../functions/shared/alerts.js";
import type { Alert, AlertDocument, AlertSetting } from "../model/types.js";
import { memoizeLast } from "./memo.js";

/**
 * Settings and fired alerts share one collection, because RxDB's open-source
 * build opens thirteen at a time and Rational would rather spend that budget
 * on the collections a household reads constantly. `kind` says which a
 * document is; these are the two views of it.
 */
export type { AlertKind } from "../../functions/shared/alerts.js";

/** The three questions a household can ask to be told about, in this order. */
export const ALERT_KINDS = [
  "large_transaction",
  "budget_exceeded",
  "low_balance",
] as const satisfies readonly AlertKind[];

export const ALERT_LABELS: Readonly<Record<AlertKind, string>> = {
  large_transaction: "A large transaction",
  budget_exceeded: "A budget gone over",
  low_balance: "An account running low",
};

/** What the threshold means for each kind, in the person's own terms. */
export const ALERT_THRESHOLD_LABELS: Readonly<Record<AlertKind, string>> = {
  large_transaction: "Tell me about spending of at least",
  budget_exceeded: "Tell me once a budget is over by",
  low_balance: "Tell me when an account falls below",
};

export function alertSettings(documents: readonly AlertDocument[]): readonly AlertSetting[] {
  return documents.filter((document): document is AlertSetting => document.kind === "setting");
}

export function settingFor(
  documents: readonly AlertDocument[],
  kind: AlertKind,
): AlertSetting | null {
  return alertSettings(documents).find((setting) => setting.alert_kind === kind) ?? null;
}

/** The history, newest first: what was fired most recently is what is read. */
export function firedAlertHistory(documents: readonly AlertDocument[]): readonly Alert[] {
  return documents
    .filter((document): document is Alert => document.kind === "alert")
    .sort((left, right) => (right.fired_at ?? 0) - (left.fired_at ?? 0));
}

export function unreadCount(documents: readonly AlertDocument[]): number {
  return firedAlertHistory(documents).filter((alert) => alert.read !== true).length;
}

export const selectAlertHistory = memoizeLast(firedAlertHistory);
