/**
 * Shared between the application and its edge functions -- see the note in
 * `rules.ts` for why this file imports nothing at all.
 *
 * Who may set a scheduled function running.
 *
 * A scheduled invocation is anonymous: the scheduler sends `x-mako-schedule-id`
 * but a stranger can send that header too, so it identifies nothing. Left
 * open, `POST /functions/v1/nightly` would let anybody on the internet run
 * every household's night at will -- idempotent, but still somebody else's
 * hand on the switch, and a free way to make the platform work.
 *
 * So the function holds a run key as a secret, and the schedule is created
 * carrying the same key in a header of its own. What the header proves is
 * only that the caller knows a value the developer configured, which is
 * exactly the claim a scheduled function needs.
 */

export const RUN_KEY_HEADER = "x-rational-run-key";

/** Constant-time within a length; a mismatched length is already public. */
export function runKeyMatches(presented: string | null, expected: string): boolean {
  if (expected === "") return false;
  if (presented === null || presented.length !== expected.length) return false;
  let difference = 0;
  for (let index = 0; index < expected.length; index += 1) {
    difference |= presented.charCodeAt(index) ^ expected.charCodeAt(index);
  }
  return difference === 0;
}
