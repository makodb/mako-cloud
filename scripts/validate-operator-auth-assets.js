#!/usr/bin/env node

import { readFile } from "node:fs/promises";
import { resolve } from "node:path";

const root = resolve(import.meta.dirname, "..");
const read = (path) => readFile(resolve(root, path), "utf8");
const [
  openapi,
  caddy,
  hostedAdapter,
  operatorAuth,
  operatorScreen,
  operatorManagement,
  managementSdk,
  config,
] = await Promise.all([
  read("api/openapi/mako-cloud-v1.yaml"),
  read("infra/ansible/roles/runtime/templates/Caddyfile.j2"),
  read("apps/console/src/hosted-operator-auth.ts"),
  read("apps/console/src/operator-auth.tsx"),
  read("apps/console/src/operator.tsx"),
  read("apps/console/src/operator-management.tsx"),
  read("packages/management-sdk/src/index.ts"),
  read("infra/ansible/roles/configuration/templates/service-config.json.j2"),
]);

for (const route of [
  "/v1/operator-auth/sessions:",
  "/v1/operator-auth/sessions/current:",
  "/v1/operator-auth/sessions/current/actions/verify-password:",
])
  assert(openapi.includes(route), `OpenAPI omits ${route}`);
for (const flag of ["Secure HttpOnly Strict", "scoped to /v1"])
  assert(openapi.includes(flag), `OpenAPI omits cookie policy ${flag}`);
for (const route of [
  "/v1/operator-auth/sessions",
  "/v1/operator-auth/sessions/current",
  "/v1/operator-auth/sessions/current/actions/verify-password",
])
  assert(caddy.includes(route), `Caddy omits ${route}`);
for (const forbidden of [
  "/_internal/v1/control/operator-entitlements/plan",
  "/_internal/v1/control/operator-entitlements/apply",
])
  assert(!caddy.includes(forbidden), `Caddy publishes protected route ${forbidden}`);

const operatorSources = [hostedAdapter, operatorAuth, operatorScreen, operatorManagement].join(
  "\n",
);
for (const forbidden of ["sessionStorage", "localStorage", "accessToken", "Bearer "])
  assert(!operatorSources.includes(forbidden), `operator console retains ${forbidden}`);
for (const required of [
  "Sign in as a platform operator",
  "Verify your operator password",
  "operator_step_up_required",
])
  assert(operatorSources.includes(required), `operator console omits ${required}`);
assert(managementSdk.includes('credentials: "include"'), "operator SDK omits cookie credentials");
for (const required of [
  '"enabled": {{ mako_operator_password_auth_enabled | bool | lower }}',
  '"break_glass_bearer_enabled": {{ mako_operator_break_glass_bearer_enabled | bool | lower }}',
])
  assert(config.includes(required), `hosted configuration omits ${required}`);

console.log(
  "validated password-backed operator API, console, private-route, and hosted config assets",
);

function assert(condition, message) {
  if (!condition) throw new Error(message);
}
