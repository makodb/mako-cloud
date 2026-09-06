import type { BudgetBucket, CategoryKind, TaxonomyEntry } from "../src/model/types.js";

export interface DefaultCategory {
  readonly id: string;
  readonly name: string;
  readonly icon: string;
  readonly bucket?: BudgetBucket;
}

export interface DefaultGroup {
  readonly id: string;
  readonly name: string;
  readonly kind: CategoryKind;
  readonly categories: readonly DefaultCategory[];
}

export function taxonomySlug(name: string): string;
export function defaultGroupId(name: string): string;
export function defaultCategoryId(name: string): string;
export const DEFAULT_TAXONOMY: readonly DefaultGroup[];
export function defaultTaxonomyDocuments(householdId: string, at: number): readonly TaxonomyEntry[];
