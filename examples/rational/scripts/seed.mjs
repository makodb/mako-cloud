#!/usr/bin/env node
/**
 * Seed the demo household with accounts, categories and tags, and about two
 * hundred transactions across three months, as an application user would:
 * signed in with the public key and pushing through the replication API.
 *
 *   node scripts/seed.mjs [--env-file mako.env.json] [--email owner@rational.test]
 *     [--password RationalDemo1!] [--household-id hh_demo] [--force]
 *
 * Re-running is a no-op once the household holds accounts, unless --force,
 * which pushes the generated documents again (existing ones are skipped as
 * conflicts).
 */
import { readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { generateDemoHousehold } from "./demo-data.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const exampleRoot = resolve(here, "..");

const DEFAULTS = {
  "env-file": join(exampleRoot, "mako.env.json"),
  endpoint: undefined,
  email: "owner@rational.test",
  password: "RationalDemo1!",
  "household-id": "hh_demo",
  currency: "USD",
  anchor: undefined,
  count: "200",
  force: false,
};

function parseArguments(argv) {
  const options = { ...DEFAULTS };
  for (let index = 0; index < argv.length; index += 1) {
    const argument = argv[index];
    const name = argument.replace(/^--/u, "");
    if (!(name in DEFAULTS)) fail(`unknown option ${argument}`);
    if (typeof DEFAULTS[name] === "boolean") {
      options[name] = true;
      continue;
    }
    options[name] = argv[index + 1];
    index += 1;
  }
  return options;
}

function fail(message) {
  console.error(`seed: ${message}`);
  process.exit(1);
}

const options = parseArguments(process.argv.slice(2));
const env = JSON.parse(readFileSync(options["env-file"], "utf8"));
const model = JSON.parse(readFileSync(join(exampleRoot, "mako", "collections.json"), "utf8"));
const endpoint = (options.endpoint ?? env.endpoint).replace(/\/$/u, "");
const base = `${endpoint}/v1/projects/${env.projectId}/environments/${env.environmentId}`;

async function request(path, body, headers = {}) {
  const response = await fetch(`${base}${path}`, {
    method: "POST",
    headers: {
      "content-type": "application/json",
      accept: "application/json",
      "x-mako-key": env.publicProjectKey,
      ...headers,
    },
    body: JSON.stringify(body),
  });
  const text = await response.text();
  let parsed = null;
  try {
    parsed = text === "" ? null : JSON.parse(text);
  } catch {
    parsed = text;
  }
  if (!response.ok) {
    throw new Error(`${path} failed with ${response.status}: ${JSON.stringify(parsed)}`);
  }
  return parsed;
}

await request("/auth/signup", { email: options.email, password: options.password }).catch(
  () => undefined,
);
const session = await request("/auth/signin", { email: options.email, password: options.password });
const authorization = { authorization: `Bearer ${session.accessToken}` };

async function pullAll(collectionId) {
  const documents = [];
  let checkpoint = null;
  for (let page = 0; page < 100; page += 1) {
    const result = await request(
      `/collections/${collectionId}/replication/pull`,
      { checkpoint, schemaVersion: model.schemaVersion, batchSize: 500 },
      authorization,
    );
    documents.push(...result.documents);
    checkpoint = result.checkpoint;
    if (result.documents.length < 500) break;
  }
  return documents.filter((document) => document._deleted !== true);
}

const householdId = options["household-id"];
const existingAccounts = (await pullAll("accounts")).filter(
  (document) => document.household_id === householdId,
);
if (existingAccounts.length > 0 && !options.force) {
  console.log(
    JSON.stringify({ householdId, seeded: false, reason: "household already has accounts" }),
  );
  process.exit(0);
}

const demo = generateDemoHousehold({
  householdId,
  currency: options.currency,
  transactionCount: Number.parseInt(options.count, 10),
  ...(options.anchor === undefined ? {} : { anchor: options.anchor }),
});

let sequence = 0;
async function push(collectionId, documents) {
  const outcome = { accepted: 0, conflict: 0, denied: 0 };
  for (let start = 0; start < documents.length; start += 100) {
    const rows = documents.slice(start, start + 100).map((document) => {
      sequence += 1;
      return {
        mutationId: `rational-seed-${document.id}-${sequence.toString(36).padStart(8, "0")}`,
        assumedMasterState: null,
        newDocumentState: { ...document, _deleted: false },
      };
    });
    const result = await request(
      `/collections/${collectionId}/replication/push`,
      { schemaVersion: model.schemaVersion, rows },
      { ...authorization, "idempotency-key": `rational-seed-${collectionId}-${start}-${Date.now()}` },
    );
    for (const row of result.outcomes) {
      outcome[row.status] = (outcome[row.status] ?? 0) + 1;
      if (row.status === "denied") {
        throw new Error(`push to ${collectionId} was denied: ${JSON.stringify(row.error)}`);
      }
    }
  }
  return outcome;
}

const summary = {
  householdId,
  seeded: true,
  accounts: await push("accounts", demo.accounts),
  taxonomy: await push("taxonomy", demo.taxonomy),
  transactions: await push("transactions", demo.transactions),
};
console.log(JSON.stringify(summary, null, 2));
