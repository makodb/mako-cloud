import { access, readFile } from "node:fs/promises";
import { resolve } from "node:path";

const root = resolve(import.meta.dirname, "..");
const matrixPath = "docs/requirements-traceability.md";
const capabilities = [
  ["CP", "openspec/specs/cloud/control-plane/spec.md"],
  ["BM", "openspec/specs/billing/metering/spec.md"],
  ["BP", "openspec/specs/billing/plans-and-entitlements/spec.md"],
  ["BI", "openspec/specs/billing/invoicing-and-balance/spec.md"],
  ["ER", "openspec/specs/functions/edge-runtime/spec.md"],
  ["PA", "openspec/specs/identity/project-auth/spec.md"],
  ["DP", "openspec/specs/security/document-policies/spec.md"],
  ["DE", "openspec/specs/storage/document-engine/spec.md"],
  ["RR", "openspec/specs/sync/rxdb-replication/spec.md"],
  ["DR", "openspec/specs/identity/developer-registration/spec.md"],
  ["LS", "openspec/specs/operations/local-bootstrap-and-smoke/spec.md"],
  ["CL", "openspec/specs/cloud/developer-cli/spec.md"],
  ["DC", "openspec/specs/cloud/developer-console/spec.md"],
  ["AF", "openspec/specs/storage/application-file-storage/spec.md"],
  ["AP", "openspec/specs/identity/auth-providers/spec.md"],
  ["DW", "openspec/specs/sync/database-webhooks/spec.md"],
  ["SF", "openspec/specs/functions/scheduled-functions/spec.md"],
  ["CD", "openspec/specs/operations/custom-domains/spec.md"],
  ["AD", "openspec/specs/cloud/api-documentation/spec.md"],
  ["RA", "openspec/specs/samples/rational-money-app/spec.md"],
];

const mockedEvidence = [
  "apps/console/test-e2e/",
  "examples/local-first/test/",
  "examples/rational/test/",
];

const matrix = await readFile(resolve(root, matrixPath), "utf8");
const rows = [
  ...matrix.matchAll(
    /^\|\s*([A-Z]{2}-\d{2})\s*\|\s*([^|]+?)\s*\|\s*([^|]+?)\s*\|\s*(Automated)\s*\|$/gm,
  ),
].map(([, id, scenario, evidence, status]) => ({
  id,
  scenario: scenario.trim(),
  evidence: evidence.trim(),
  status,
}));

const expected = [];
for (const [prefix, specPath] of capabilities) {
  const spec = await readFile(resolve(root, specPath), "utf8");
  const scenarios = [...spec.matchAll(/^#### Scenario: (.+)$/gm)].map((match) => match[1].trim());
  scenarios.forEach((scenario, index) => {
    expected.push({ id: `${prefix}-${String(index + 1).padStart(2, "0")}`, scenario });
  });
}

if (rows.length !== expected.length) {
  throw new Error(
    `traceability row count ${rows.length} does not match ${expected.length} scenarios`,
  );
}

const rowIds = new Set();
for (const row of rows) {
  if (rowIds.has(row.id)) throw new Error(`duplicate traceability ID ${row.id}`);
  rowIds.add(row.id);

  const requirement = expected.find(({ id }) => id === row.id);
  if (requirement === undefined) throw new Error(`unexpected traceability ID ${row.id}`);
  if (row.scenario !== requirement.scenario) {
    throw new Error(
      `${row.id} is ${JSON.stringify(row.scenario)}, expected ${JSON.stringify(requirement.scenario)}`,
    );
  }

  const references = [...row.evidence.matchAll(/`([^`]+)`/g)].map((match) => match[1]);
  if (references.length === 0) throw new Error(`${row.id} has no evidence reference`);
  const mocked = references.filter((reference) =>
    mockedEvidence.some((p) => reference.includes(p)),
  );
  if (mocked.length > 0 && !row.evidence.includes("(mocked backend)")) {
    throw new Error(
      `${row.id} cites a mocked-backend test (${mocked.join(", ")}) without marking it "(mocked backend)"`,
    );
  }
  for (const reference of references) {
    const file = reference.split("::", 1)[0];
    if (file.startsWith("/") || file.split("/").includes("..")) {
      throw new Error(`${row.id} has a non-repository evidence path: ${file}`);
    }
    await access(resolve(root, file));
  }
}

for (const requirement of expected) {
  if (!rowIds.has(requirement.id)) throw new Error(`missing traceability row ${requirement.id}`);
}

console.log(
  `validated ${rows.length} automated scenarios across ${capabilities.length} capability specs`,
);
