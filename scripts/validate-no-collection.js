// The beta shows bills and collects nothing. "We did not build collection"
// degrades quietly -- a well-meaning dependency or a quota path that starts
// reading the balance turns an informational number into a restriction on
// someone who was told they would not be charged. So the absence is checked,
// not assumed.
import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

const root = resolve(import.meta.dirname, "..");

function fail(message) {
  console.error(`no-collection: ${message}`);
  process.exit(1);
}

// 1. No payment-provider integration anywhere: no SDK dependency, no provider
//    endpoint, no card-data field names in source.
const providerMarkers = [
  "api.stripe.com",
  "js.stripe.com",
  "stripe-rust",
  "braintree",
  "paddle.com",
  "checkout.com",
  "adyen",
  "pk_live_",
  "sk_live_",
  "card_number",
  "cardNumber",
  "cvv",
  "cvc_check",
];
const lockfiles = ["Cargo.lock", "package-lock.json"];
for (const lockfile of lockfiles) {
  const contents = readFileSync(resolve(root, lockfile), "utf8").toLowerCase();
  for (const marker of ["stripe", "braintree", "adyen", "paddle"]) {
    if (contents.includes(marker)) {
      fail(`${lockfile} carries a payment-provider dependency (${marker})`);
    }
  }
}
const tracked = execFileSync("git", ["ls-files", "crates", "services", "packages", "apps"], {
  cwd: root,
  encoding: "utf8",
})
  .split("\n")
  .filter((path) => /\.(rs|ts|tsx|mjs|js)$/.test(path))
  .filter((path) => !path.includes("generated"));
for (const path of tracked) {
  const contents = readFileSync(resolve(root, path), "utf8");
  for (const marker of providerMarkers) {
    if (contents.includes(marker)) {
      fail(`${path} references a payment marker (${marker})`);
    }
  }
}

// 2. No enforcement path reads the balance. The balance may appear only where
//    it is computed and shown; a quota, suspension, or lifecycle decision that
//    consults it would restrict service over an informational number.
const balanceReaders = tracked.filter((path) => {
  const contents = readFileSync(resolve(root, path), "utf8");
  return /balance_micro_dollars|balanceMicroDollars/i.test(contents);
});
const allowedBalanceReaders = new Set([
  "services/mako-control-plane/src/management_http.rs",
  "crates/mako-smoke/tests/telemetry_pipeline.rs",
  "packages/management-sdk/src/index.ts",
  // The console page that shows the bill is the surface the balance exists
  // for; its e2e spec asserts what that page renders.
  "apps/console/src/billing.tsx",
  // The usage screen repeats the bill summary beside the quantities it explains;
  // it renders the balance and takes no decision from it.
  "apps/console/src/usage.tsx",
  "apps/console/test-e2e/management-workflows.spec.ts",
  // Fixtures that mock the bill response for the screens above.
  "apps/console/test-e2e/home-dashboard.spec.ts",
  "apps/console/test-e2e/surfaced-capabilities.spec.ts",
  "apps/console/test-e2e/usage-activity.spec.ts",
]);
for (const path of balanceReaders) {
  if (!allowedBalanceReaders.has(path)) {
    fail(
      `${path} reads the balance; only the surface that shows it may. ` +
        "A quota or lifecycle decision must never consult it.",
    );
  }
}

console.log(
  `no-collection: verified ${tracked.length} source files carry no payment integration ` +
    "and no enforcement path reads the balance",
);
