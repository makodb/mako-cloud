import type {
  Account,
  Budget,
  Goal,
  Household,
  NetWorthSnapshot,
  Recurrence,
  Rule,
  TaxonomyEntry,
  Transaction,
} from "../src/model/types.js";

export interface DemoHouseholdOptions {
  readonly householdId: string;
  readonly currency?: string;
  readonly anchor?: string;
  readonly transactionCount?: number;
  readonly ownerId?: string;
  readonly name?: string;
}

/** Every household collection the demo fills, keyed as the collections are. */
export interface DemoHousehold {
  readonly household: Household;
  readonly accounts: Account[];
  readonly taxonomy: TaxonomyEntry[];
  readonly transactions: Transaction[];
  readonly rules: Rule[];
  readonly budgets: Budget[];
  readonly recurrences: Recurrence[];
  readonly goals: Goal[];
  readonly net_worth_snapshots: NetWorthSnapshot[];
}

export function generateDemoHousehold(options: DemoHouseholdOptions): DemoHousehold;
