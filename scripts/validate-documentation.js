#!/usr/bin/env node

import { access, readFile, readdir } from "node:fs/promises";
import { dirname, extname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { headingAnchors } from "./markdown-anchors-lib.js";

const root = resolve(fileURLToPath(new URL("..", import.meta.url)));
const indexPath = "docs/README.md";
const userBook = "docs/user-book.md";
const devBook = "docs/dev-book.md";

// Every product area must be documented in a named section of one of the two
// books, and its evidence must exist. A section is named by its heading text;
// the check is on the heading, so a renamed chapter is a visible decision.
const coverage = [
  {
    area: "local development",
    sections: [
      [devBook, "Local development"],
      [devBook, "Configuration reference"],
    ],
    evidence: ["scripts/local/prepare.sh", "crates/mako-config/src/lib.rs"],
  },
  {
    area: "deployment",
    sections: [[devBook, "Deployment"]],
    evidence: [
      "infra/production/storage-statefulsets.yaml",
      "scripts/validate-production-storage.js",
    ],
  },
  {
    area: "operations",
    sections: [
      [devBook, "Production RocksDB operations"],
      [devBook, "Observability"],
      [devBook, "Runbooks"],
      [devBook, "Release gates"],
      [devBook, "Rollback qualification"],
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
    sections: [
      [devBook, "The threat model"],
      [devBook, "Tenant-boundary qualification"],
    ],
    evidence: [
      "scripts/run-tenant-boundary-qualification.sh",
      "crates/mako-audit/tests/threat_model.rs",
    ],
  },
  {
    area: "end-to-end smoke",
    sections: [[devBook, "The smoke suites"]],
    evidence: ["scripts/run-e2e-smoke-qualification.sh", "crates/mako-smoke/tests/happy_path.rs"],
  },
  {
    area: "API",
    sections: [[userBook, "The public API and SDKs"]],
    evidence: ["api/openapi/mako-cloud-v1.yaml", "packages/management-sdk/test/client.test.mjs"],
  },
  {
    area: "RxDB",
    sections: [
      [userBook, "Building a local-first app with RxDB"],
      [userBook, "The replication protocol"],
    ],
    evidence: [
      "examples/local-first/README.md",
      "scripts/run-rxdb-chaos-qualification.sh",
      "examples/local-first/test/local-first.spec.ts",
    ],
  },
  {
    area: "document policy",
    sections: [[userBook, "Document policies"]],
    evidence: ["scripts/run-policy-security-qualification.sh", "crates/mako-policy/src/store.rs"],
  },
  {
    area: "project authentication",
    sections: [[userBook, "Application authentication"]],
    evidence: [
      "scripts/run-auth-security-qualification.sh",
      "crates/mako-identity/src/signing_keys.rs",
    ],
  },
  {
    area: "edge functions",
    sections: [
      [userBook, "Edge functions"],
      [userBook, "Serving a function locally"],
      [devBook, "The edge runtime: pin and protocol"],
    ],
    evidence: [
      "scripts/run-edge-security-qualification.sh",
      "scripts/run-edge-e2e-qualification.sh",
      "crates/mako-smoke/tests/edge_function.rs",
      "packages/cli/test/compatibility.integration.mjs",
    ],
  },
  {
    area: "sample application",
    sections: [
      [userBook, "Sample applications"],
      [devBook, "The sample applications as platform gates"],
    ],
    evidence: [
      "scripts/run-rational-smoke-qualification.sh",
      "crates/mako-smoke/tests/rational.rs",
      "examples/rational/PLATFORM-FINDINGS.md",
    ],
  },
];

const index = await readFile(resolve(root, indexPath), "utf8");
for (const book of [userBook, devBook]) {
  const relative = book.replace(/^docs\//, "");
  if (!index.includes(`(${relative})`)) {
    throw new Error(`documentation index does not publish ${book}`);
  }
}

const headings = new Map();
for (const book of [userBook, devBook]) {
  headings.set(book, headingAnchors(await readFile(resolve(root, book), "utf8")));
}

for (const { area, sections, evidence } of coverage) {
  for (const path of evidence) await requirePath(path, `${area} coverage`);
  for (const [book, heading] of sections) {
    if (!headings.get(book).texts.has(heading)) {
      throw new Error(`${book} has no "${heading}" section for ${area} coverage`);
    }
  }
}

// Every relative link in the published Markdown must resolve, and every
// fragment into one of the books must name a heading that exists there.
const markdownFiles = [resolve(root, "README.md"), ...(await markdownBelow(resolve(root, "docs")))];
let links = 0;
for (const absolutePath of markdownFiles) {
  const source = await readFile(absolutePath, "utf8");
  const own = headingAnchors(source);
  for (const match of source.matchAll(/\[[^\]]*\]\(([^)]+)\)/g)) {
    const raw = match[1].trim().replace(/^<|>$/g, "");
    if (raw === "" || /^(?:https?:|mailto:)/.test(raw)) continue;
    const [target, fragment] = raw.split("#", 2);
    const targetPath =
      target === "" ? absolutePath : resolve(dirname(absolutePath), decodeURIComponent(target));
    await access(targetPath).catch(() => {
      throw new Error(`${relativeTo(absolutePath)} has a broken link: ${match[1]}`);
    });
    if (fragment !== undefined && extname(targetPath) === ".md") {
      const { anchors } = target === "" ? own : headingAnchors(await readFile(targetPath, "utf8"));
      if (!anchors.has(fragment)) {
        throw new Error(`${relativeTo(absolutePath)} links to a missing section: ${match[1]}`);
      }
    }
    links += 1;
  }
}

console.log(
  `validated ${coverage.length} documented areas, ${markdownFiles.length} Markdown files, and ${links} links`,
);

function relativeTo(absolutePath) {
  return absolutePath.slice(root.length + 1);
}

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
