import assert from "node:assert/strict";
import { test } from "node:test";

import { cleanDescription, mergePlan, resolveMerchant } from "../functions/shared/merchants.ts";
import { normalizeDescription } from "../functions/shared/recurrences.ts";

const MERCHANTS = [
  { id: "m_grocer", name: "Corner Grocer", patterns: ["sq corner grocer"] },
  { id: "m_spotify", name: "Spotify", patterns: ["paypal spotify"] },
  { id: "m_bare", name: "Bare" },
];

test("a statement line cleans up into a name", () => {
  assert.equal(cleanDescription(normalizeDescription("SQ *CORNER GROCER 0412")), "Corner Grocer");
  assert.equal(cleanDescription(normalizeDescription("PAYPAL *SPOTIFY")), "Spotify");
  // The processor's own name is kept when it is all there is.
  assert.equal(cleanDescription(normalizeDescription("PAYPAL")), "Paypal");
  // The possessive's stray letter goes with its apostrophe.
  assert.equal(cleanDescription(normalizeDescription("MCDONALD'S #4471")), "Mcdonald");
  assert.equal(cleanDescription(""), "");
});

test("the merchant on the transaction wins, then a pattern, then the cleaned text", () => {
  const chosen = resolveMerchant(
    { merchant_id: "m_spotify", description: "SQ *CORNER GROCER 0412" },
    MERCHANTS,
    normalizeDescription,
  );
  assert.deepEqual(chosen, { id: "m_spotify", name: "Spotify" });

  const byPattern = resolveMerchant(
    { description: "SQ *CORNER GROCER 0412" },
    MERCHANTS,
    normalizeDescription,
  );
  assert.deepEqual(byPattern, { id: "m_grocer", name: "Corner Grocer" });

  // A stored normalized description is used as it is.
  const stored = resolveMerchant(
    { description: "anything", normalized_description: "paypal spotify" },
    MERCHANTS,
    normalizeDescription,
  );
  assert.deepEqual(stored, { id: "m_spotify", name: "Spotify" });

  const unknown = resolveMerchant({ description: "BLUE BOTTLE #221" }, MERCHANTS, normalizeDescription);
  assert.deepEqual(unknown, { id: null, name: "Blue Bottle" });
});

test("a merchant id pointing nowhere falls through, and an empty description stays empty", () => {
  const dangling = resolveMerchant(
    { merchant_id: "m_gone", description: "SQ *CORNER GROCER 0412" },
    MERCHANTS,
    normalizeDescription,
  );
  assert.deepEqual(dangling, { id: "m_grocer", name: "Corner Grocer" });
  assert.deepEqual(resolveMerchant({ description: "  " }, MERCHANTS, normalizeDescription), {
    id: null,
    name: "",
  });
});

test("merging keeps the winner's name, takes the loser's patterns, and refiles the transactions", () => {
  const loser = { id: "m_old", name: "Corner Grocer (old)", patterns: ["corner grocer", "sq corner grocer"] };
  const plan = mergePlan(loser, MERCHANTS[0]);
  assert.deepEqual(plan, {
    winnerPatch: { patterns: ["sq corner grocer", "corner grocer"] },
    transactionPatch: { merchant_id: "m_grocer" },
  });
  assert.deepEqual(mergePlan({ id: "a", name: "A" }, { id: "b", name: "B" }).winnerPatch.patterns, []);
});
