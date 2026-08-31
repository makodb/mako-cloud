import assert from "node:assert/strict";
import { test } from "node:test";

import {
  minorUnitsFromPlaid,
  parseExchange,
  parseLinkToken,
  parseSyncPage,
  plaidConnectionBody,
  plaidItemBody,
  removedIds,
  syncEntries,
} from "../functions/shared/plaid.ts";

/**
 * The fixtures are the shapes Plaid's sandbox actually answers with --
 * trimmed to the fields the engine reads, but never reshaped. What these
 * tests guard is the two translations that could silently corrupt money
 * (sign and units) and the one rule that could silently leak a credential
 * (what goes in the connection a household can read).
 */

function sandboxTransaction(overrides = {}) {
  return {
    transaction_id: "lPNjeW1nR6CDn5okmGQ6hEpMo4lLNoSrzqDje",
    account_id: "BxBXxLj1m4HMXBm9WZZmCWVbPjX16EHwv99vp",
    amount: 89.4,
    iso_currency_code: "USD",
    unofficial_currency_code: null,
    date: "2026-08-28",
    name: "SparkFun",
    merchant_name: "SparkFun",
    pending: false,
    payment_channel: "in store",
    category: ["Shops", "Computers and Electronics"],
    ...overrides,
  };
}

function sandboxPage(overrides = {}) {
  return {
    added: [],
    modified: [],
    removed: [],
    next_cursor: "CAESJmVoTjNPVEV5TvNRk8eEQm5eb2trbUdRNmhF",
    has_more: false,
    request_id: "45QSn",
    transactions_update_status: "HISTORICAL_UPDATE_COMPLETE",
    ...overrides,
  };
}

test("Plaid's positive-outflow major units become Rational's negative minor units", () => {
  assert.equal(minorUnitsFromPlaid(89.4), -8940);
  assert.equal(minorUnitsFromPlaid(-500), 50_000, "a deposit becomes positive money arriving");
  assert.equal(minorUnitsFromPlaid(12.34), -1234, "float representation must not shave a cent");
  assert.equal(minorUnitsFromPlaid(0), 0);
});

test("a sync page's added and modified entries become statement upserts", () => {
  const page = parseSyncPage(
    sandboxPage({
      added: [sandboxTransaction()],
      modified: [
        sandboxTransaction({
          transaction_id: "corrected-1",
          amount: 12.5,
          merchant_name: null,
          name: "UBER TRIP HELP.UBER.COM",
          iso_currency_code: null,
        }),
      ],
    }),
  );
  const entries = syncEntries(page, "USD");
  assert.equal(entries.length, 2);
  assert.deepEqual(entries[0], {
    externalId: "lPNjeW1nR6CDn5okmGQ6hEpMo4lLNoSrzqDje",
    date: "2026-08-28",
    amount: -8940,
    currency: "USD",
    description: "SparkFun",
  });
  assert.equal(entries[1].description, "UBER TRIP HELP.UBER.COM", "no merchant name falls back");
  assert.equal(entries[1].currency, "USD", "no currency falls back to the account's");
});

test("the same transaction added and modified in one page is written once, last word winning", () => {
  const page = parseSyncPage(
    sandboxPage({
      added: [sandboxTransaction({ amount: 10 })],
      modified: [sandboxTransaction({ amount: 11.25 })],
    }),
  );
  const entries = syncEntries(page, "USD");
  assert.equal(entries.length, 1);
  assert.equal(entries[0].amount, -1125);
});

test("a pending charge that posts is one removal and one addition, not a double", () => {
  const pendingId = "pending-roof-repair";
  const page = parseSyncPage(
    sandboxPage({
      added: [
        sandboxTransaction({
          transaction_id: "posted-roof-repair",
          pending: false,
          pending_transaction_id: pendingId,
        }),
      ],
      removed: [{ transaction_id: pendingId }],
    }),
  );
  assert.deepEqual(removedIds(page), [pendingId]);
  const entries = syncEntries(page, "USD");
  assert.equal(entries.length, 1);
  assert.equal(entries[0].externalId, "posted-roof-repair");
});

test("a malformed page is refused, never partially read", () => {
  assert.throws(() => parseSyncPage({ added: [], modified: [], removed: [] }), /sync page/u);
  assert.throws(
    () => parseSyncPage(sandboxPage({ added: [{ transaction_id: "" }] })),
    /transaction/u,
  );
  assert.throws(() => parseSyncPage(sandboxPage({ removed: [{}] })), /removal/u);
  assert.throws(
    () => parseSyncPage(sandboxPage({ added: [sandboxTransaction({ amount: Number.NaN })] })),
    /transaction/u,
  );
});

test("token responses are read strictly", () => {
  assert.equal(
    parseLinkToken({ link_token: "link-sandbox-af1a0311", expiration: "2026-08-31T12:56:34Z" }),
    "link-sandbox-af1a0311",
  );
  assert.throws(() => parseLinkToken({ link_token: "" }), /link token/u);
  assert.deepEqual(
    parseExchange({ access_token: "access-sandbox-de3ce8ef", item_id: "M5eVJqLnv3tbzdngLDp9FL5OlDNxlNhlE55op" }),
    { itemId: "M5eVJqLnv3tbzdngLDp9FL5OlDNxlNhlE55op", accessToken: "access-sandbox-de3ce8ef" },
  );
  assert.throws(() => parseExchange({ item_id: "x" }), /exchange/u);
});

test("the connection a household reads carries no credential; the item record carries exactly one", () => {
  const connection = plaidConnectionBody({
    connectionId: "con_1",
    householdId: "hh_1",
    accountId: "acct_1",
    itemId: "item-1",
    institutionName: "First Platypus Bank",
    now: 1_000,
  });
  const encoded = JSON.stringify(connection);
  assert.equal(connection.kind, "plaid");
  assert.ok(!encoded.includes("access"), `no token-shaped field may appear: ${encoded}`);
  assert.ok(!encoded.includes("token"), `no token-shaped field may appear: ${encoded}`);
  assert.ok(!encoded.includes("secret"), `no secret-shaped field may appear: ${encoded}`);

  const item = plaidItemBody({
    connectionId: "con_1",
    householdId: "hh_1",
    itemId: "item-1",
    accessToken: "access-sandbox-de3ce8ef",
    cursor: "",
    now: 1_000,
  });
  assert.equal(item.access_token, "access-sandbox-de3ce8ef");
  assert.equal(item.id, "con_1", "the item is keyed by its connection");
});
