import type { Account, Household, TaxonomyEntry, Transaction } from "../src/model/types.js";

export interface DemoHouseholdOptions {
  readonly householdId: string;
  readonly currency?: string;
  readonly anchor?: string;
  readonly transactionCount?: number;
  readonly ownerId?: string;
  readonly name?: string;
}

export interface DemoHousehold {
  readonly household: Household;
  readonly accounts: Account[];
  readonly taxonomy: TaxonomyEntry[];
  readonly transactions: Transaction[];
}

export function generateDemoHousehold(options: DemoHouseholdOptions): DemoHousehold;
