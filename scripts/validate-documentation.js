#!/usr/bin/env node

import { access, readFile, readdir } from "node:fs/promises";
import { dirname, extname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(fileURLToPath(new URL("..", import.meta.url)));
const indexPath = "docs/README.md";
const coverage = [
  {
    area: "local development",
    guides: ["docs/local-development.md", "docs/configuration.md"],
    evidence: ["scripts/local/prepare.sh", "crates/mako-config/src/lib.rs"],
  },
  {
    area: "deployment",
    guides: ["docs/deployment.md"],
    evidence: [
      "infra/production/storage-statefulsets.yaml",
      "scripts/validate-production-storage.js",
    ],
  },
  {
    area: "operations",
    guides: [
      "docs/production-rocksdb-operations.md",
      "docs/observability.md",
      "docs/runbooks/README.md",
      "docs/release-gates.md",
      "docs/rollback-qualification.md",
    ],
    evidence: [
      "scripts/validate-observability-assets.js",
      "scripts/validate-production-release.js",
      "scripts/validate-release-gates.js",
      "scripts/validate-rollback-qualification.js",
    ],
  },
  {
    area: "security",
    guides: ["docs/threat-model.md", "docs/tenant-boundary-qualification.md"],
    evidence: [
      "scripts/run-tenant-boundary-qualification.sh",
      "crates/mako-audit/tests/threat_model.rs",
    ],
  },
  {
    area: "end-to-end smoke",
    guides: ["docs/e2e-smoke.md"],
    evidence: ["scripts/run-e2e-smoke-qualification.sh", "crates/mako-smoke/tests/happy_path.rs"],
  },
  {
    area: "API",
    guides: ["docs/api.md"],
    evidence: ["api/openapi/mako-cloud-v1.yaml", "packages/management-sdk/test/client.test.mjs"],
  },
  {
    area: "RxDB",
    guides: ["docs/rxdb-client.md", "examples/local-first/README.md"],
    evidence: [
      "scripts/run-rxdb-chaos-qualification.sh",
      "examples/local-first/test/local-first.spec.ts",
    ],
  },
  {
    area: "document policy",
    guides: ["docs/document-policies.md"],
    evidence: ["scripts/run-policy-security-qualification.sh", "crates/mako-policy/src/store.rs"],
  },
  {
    area: "project authentication",
    guides: ["docs/project-auth.md"],
    evidence: [
      "scripts/run-auth-security-qualification.sh",
      "crates/mako-identity/src/signing_keys.rs",
    ],
  },
  {
    area: "edge functions",
    guides: ["docs/edge-functions.md", "docs/local-functions.md", "docs/edge-runtime-protocol.md"],
    evidence: [
      "scripts/run-edge-security-qualification.sh",
      "scripts/run-edge-e2e-qualification.sh",
      "crates/mako-smoke/tests/edge_function.rs",
      "packages/cli/test/compatibility.integration.mjs",
    ],
  },
  {
    area: "sample application",
    guides: ["docs/rational.md"],
    evidence: [
      "scripts/run-rational-smoke-qualification.sh",
      "crates/mako-smoke/tests/rational.rs",
      "examples/rational/PLATFORM-FINDINGS.md",
    ],
  },
];

const index = await readFile(resolve(root, indexPath), "utf8");
for (const { area, guides, evidence } of coverage) {
  for (const path of [...guides, ...evidence]) await requirePath(path, `${area} coverage`);
  for (const guide of guides) {
    const relative = guide.replace(/^docs\//, "");
    if (!index.includes(`(${relative})`) && !index.includes(`(../${relative})`)) {
      throw new Error(`documentation index does not publish ${guide}`);
    }
  }
}

const markdownFiles = ["README.md", ...(await markdownBelow(resolve(root, "docs")))];
for (const absolutePath of markdownFiles.map((path) =>
  path === "README.md" ? resolve(root, path) : path,
)) {
  const source = await readFile(absolutePath, "utf8");
  for (const match of source.matchAll(/\[[^\]]*\]\(([^)]+)\)/g)) {
    const target = match[1].trim().replace(/^<|>$/g, "").split("#", 1)[0];
    if (target === "" || /^(?:https?:|mailto:)/.test(target)) continue;
    await access(resolve(dirname(absolutePath), decodeURIComponent(target))).catch(() => {
      throw new Error(`${absolutePath.slice(root.length + 1)} has a broken link: ${match[1]}`);
    });
  }
}

console.log(
  `validated ${coverage.length} published documentation areas and ${markdownFiles.length} Markdown files`,
);

async function requirePath(path, context) {
  await access(resolve(root, path)).catch(() => {
    throw new Error(`${context} is missing ${path}`);
  });
}

async function markdownBelow(directory) {
  const paths = [];
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const path = resolve(directory, entry.name);
    if (entry.isDirectory()) paths.push(...(await markdownBelow(path)));
    if (entry.isFile() && extname(entry.name) === ".md") paths.push(path);
  }
  return paths;
}
