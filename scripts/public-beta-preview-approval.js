#!/usr/bin/env node

import { readFileSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import {
  buildApproval,
  derivePreviewQualification,
  requiredConfirmation,
  validateApproval,
} from "./public-beta-preview-approval-lib.js";

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const [command, ...arguments_] = process.argv.slice(2);
const options = parseOptions(arguments_);

if (command === "plan") {
  const operatorIdentity = requiredOption(options, "operator");
  assert(
    options.expiresAt === undefined,
    "--expires-at is not supported; persistent preview acceptance is manually paused",
  );
  const acceptedAt = options.acceptedAt ?? new Date().toISOString();
  const qualification = derivePreviewQualification(repositoryRoot);
  const plan = {
    schemaVersion: 2,
    kind: "risk-accepted-public-preview-plan",
    persistence: "until-manually-paused",
    operatorIdentity,
    ...qualification,
    acceptedAt,
    expiresAt: null,
    requiredConfirmation: requiredConfirmation({
      releaseDigest: qualification.releaseDigest,
      planHash: qualification.planHash,
      blockerDigest: qualification.blockerDigest,
    }),
  };
  writeOrPrint(options.output, plan);
  process.exit(0);
}

if (command === "approve") {
  const planPath = requiredOption(options, "plan");
  const typedConfirmation = requiredOption(options, "confirm");
  const plan = JSON.parse(readFileSync(resolve(repositoryRoot, planPath), "utf8"));
  const qualification = derivePreviewQualification(repositoryRoot);
  assert(plan.schemaVersion === 2, "preview plan schema is invalid");
  assert(plan.kind === "risk-accepted-public-preview-plan", "preview plan kind is invalid");
  assert(plan.persistence === "until-manually-paused", "preview persistence changed");
  assert(plan.expiresAt === null, "persistent preview plan must not expire");
  assert(plan.planHash === qualification.planHash, "preview plan binding changed");
  assert(plan.releaseDigest === qualification.releaseDigest, "preview release binding changed");
  assert(plan.blockerDigest === qualification.blockerDigest, "preview blocker binding changed");
  assert(typedConfirmation === plan.requiredConfirmation, "typed confirmation does not match plan");
  const approval = buildApproval({
    operatorIdentity: plan.operatorIdentity,
    planHash: plan.planHash,
    releaseDigest: plan.releaseDigest,
    blockers: plan.blockers,
    acceptedAt: plan.acceptedAt,
    nonWaivableSafeguards: qualification.nonWaivableSafeguards,
    typedConfirmation,
  });
  writeOrPrint(options.output, approval);
  process.exit(0);
}

if (command === "verify") {
  const approvalPath = requiredOption(options, "approval");
  const qualification = derivePreviewQualification(repositoryRoot);
  const approval = JSON.parse(readFileSync(resolve(repositoryRoot, approvalPath), "utf8"));
  validateApproval(approval, {
    expectedPlanHash: qualification.planHash,
    expectedReleaseDigest: qualification.releaseDigest,
    expectedBlockers: qualification.blockers,
    now: options.now ?? new Date().toISOString(),
  });
  console.log(
    JSON.stringify({
      valid: true,
      kind: approval.kind,
      operatorIdentity: approval.operatorIdentity,
      releaseDigest: approval.releaseDigest,
      planHash: approval.planHash,
      blockerDigest: approval.blockerDigest,
      persistence: approval.persistence,
      expiresAt: approval.expiresAt,
    }),
  );
  process.exit(0);
}

throw new Error("usage: public-beta-preview-approval.js <plan|approve|verify> [options]");

function parseOptions(arguments_) {
  const parsed = {};
  for (let index = 0; index < arguments_.length; index += 1) {
    const argument = arguments_[index];
    assert(argument.startsWith("--"), `unexpected argument: ${argument}`);
    const key = argument.slice(2).replaceAll(/-([a-z])/g, (_, letter) => letter.toUpperCase());
    const value = arguments_[index + 1];
    assert(value !== undefined && !value.startsWith("--"), `missing value for ${argument}`);
    parsed[key] = value;
    index += 1;
  }
  return parsed;
}

function requiredOption(options, name) {
  const value = options[name];
  assert(typeof value === "string" && value.length > 0, `--${name} is required`);
  return value;
}

function writeOrPrint(output, value) {
  const serialized = `${JSON.stringify(value, null, 2)}\n`;
  if (output) writeFileSync(resolve(repositoryRoot, output), serialized, { mode: 0o600 });
  else process.stdout.write(serialized);
}

function assert(condition, message) {
  if (!condition) throw new Error(message);
}
