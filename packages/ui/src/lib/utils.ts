import { type ClassValue, clsx } from "clsx";
import { twMerge } from "tailwind-merge";

/**
 * Join class names and let a later Tailwind utility win over an earlier one of
 * the same kind, so a caller can override a component's defaults with its own
 * `className` instead of fighting them.
 */
export function cn(...inputs: ClassValue[]): string {
  return twMerge(clsx(inputs));
}
