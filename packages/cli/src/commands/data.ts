import { createHash } from "node:crypto";
import { createReadStream, createWriteStream } from "node:fs";
import { rename, stat, unlink } from "node:fs/promises";
import { Readable, Transform, Writable } from "node:stream";
import { pipeline } from "node:stream/promises";
import type { ReadableStream as WebReadableStream } from "node:stream/web";
import { StringDecoder } from "node:string_decoder";

import {
  type ArtifactGrant,
  type DataJob,
  type DataJobCreateRequest,
  type MakoManagementClient,
  ManagementApiError,
} from "@mako-cloud/management-sdk";

import type { CommandContext } from "../cli/context.js";
import { CliError, EXIT, usageError } from "../cli/errors.js";
import { CLI_NAME } from "../cli/name.js";
import type { TableColumn } from "../cli/output.js";
import type { Command, CommandArgs, OptionSpec } from "../cli/registry.js";
import { TENANT_OPTIONS, type Tenant, tenantFrom } from "./shared.js";

type JobState = DataJob["state"];
type ConflictStrategy = NonNullable<DataJobCreateRequest["conflictStrategy"]>;

/**
 * `DataJob.state` in api/openapi/mako-cloud-v1.yaml is the enum
 *   awaiting_upload | dry_run | awaiting_confirmation | queued | running |
 *   cancelling | succeeded | failed | cancelled | expired
 * An import moves awaiting_upload -> dry_run (once the artifact is uploaded)
 * -> awaiting_confirmation (the dry run completes synchronously and sets the
 * manifest) -> queued (confirmed) -> running -> a terminal state. An export
 * starts at queued. A download grant is only issued for a `succeeded` export;
 * partial artifacts are never served.
 */
const TERMINAL_STATES: ReadonlySet<JobState> = new Set<JobState>([
  "succeeded",
  "failed",
  "cancelled",
  "expired",
]);
const IN_FLIGHT_STATES: ReadonlySet<JobState> = new Set<JobState>([
  "queued",
  "running",
  "cancelling",
]);
const CONFLICT_STRATEGIES: readonly ConflictStrategy[] = [
  "create_only",
  "update_existing",
  "upsert",
];

const JOB_COLUMNS: readonly TableColumn[] = [
  { key: "jobId" },
  { key: "kind" },
  { key: "state" },
  { key: "collectionId" },
  { key: "conflictStrategy", label: "strategy" },
  { key: "createdAtUnixSeconds", label: "createdAt" },
];

const JOB_POSITIONAL = { name: "job-id", description: "Data job id", required: true };

const COLLECTION_OPTION: Readonly<Record<string, OptionSpec>> = {
  collection: {
    type: "string",
    description: "Collection id",
    placeholder: "<collection-id>",
  },
};

const JOB_OPTION: Readonly<Record<string, OptionSpec>> = {
  job: {
    type: "string",
    description: "Resume an existing job instead of creating one",
    placeholder: "<job-id>",
  },
};

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function describeJob(job: DataJob): string {
  const progress = job.progress;
  const detail =
    job.kind === "export"
      ? `exported ${progress.exported}, ${progress.bytes} bytes`
      : `processed ${progress.processed}, committed ${progress.committed}, failed ${progress.failed}, skipped ${progress.skipped}`;
  return `job ${job.jobId}: ${job.state} (${detail})`;
}

function jobFailed(job: DataJob): CliError {
  const errors = job.errors.slice(0, 5).join("; ");
  return new CliError(
    `${job.kind} job ${job.jobId} ${job.state}${errors === "" ? "" : `: ${errors}`}`,
    EXIT.api,
    "CLI_JOB_FAILED",
  );
}

function tenantFlags(tenant: Tenant): string {
  return `--project ${tenant.projectId} --env ${tenant.environmentId}`;
}

/** Polls an in-flight job to a terminal state; a settled job is returned as is. */
function settle(
  context: CommandContext,
  client: MakoManagementClient,
  tenant: Tenant,
  job: DataJob,
): Promise<DataJob> {
  if (!IN_FLIGHT_STATES.has(job.state)) return Promise.resolve(job);
  return context.waitFor(
    () => client.getDataJob(tenant.projectId, tenant.environmentId, job.jobId),
    (current) => TERMINAL_STATES.has(current.state),
    describeJob,
  );
}

// ---- artifact transfer ------------------------------------------------------
//
// The API has no SDK method for the artifact itself: a grant names a URL whose
// query carries a one-time grant token, uploads are `PUT` with a `Digest`
// header of the form `sha-256=<hex>`, downloads are `GET` answered with the
// same header. Bodies are streamed through node:fs so a 512 MiB artifact is
// never held in memory. Like the console, the request carries the bearer
// credential and resolves the URL against the endpoint.

async function artifactRequest(
  context: CommandContext,
  grant: ArtifactGrant,
  init: RequestInit,
): Promise<Response> {
  const endpoint = await context.endpoint();
  const credential = await context.credential();
  const headers = new Headers(init.headers);
  headers.set("Authorization", `Bearer ${credential.accessToken}`);
  headers.set("Origin", new URL(endpoint).origin);
  const fetchImpl = context.io.fetch ?? globalThis.fetch;
  // The URL holds the grant token: it is used here and never printed.
  const response = await fetchImpl(new URL(grant.url, endpoint), { ...init, headers });
  if (!response.ok) throw await artifactError(response);
  return response;
}

async function artifactError(response: Response): Promise<Error> {
  let body: unknown;
  try {
    body = await response.json();
  } catch {
    body = undefined;
  }
  const error = isRecord(body) ? body.error : undefined;
  if (isRecord(error) && typeof error.code === "string" && typeof error.message === "string") {
    return new ManagementApiError(
      error as unknown as ConstructorParameters<typeof ManagementApiError>[0],
      response.status,
    );
  }
  return new CliError(
    `artifact transfer failed with HTTP ${response.status}`,
    EXIT.api,
    "CLI_ARTIFACT_TRANSFER",
  );
}

async function sha256File(
  path: string,
): Promise<{ readonly digest: string; readonly bytes: number }> {
  const hash = createHash("sha256");
  let bytes = 0;
  const source: AsyncIterable<Buffer> = createReadStream(path);
  for await (const chunk of source) {
    hash.update(chunk);
    bytes += chunk.length;
  }
  return { digest: hash.digest("hex"), bytes };
}

async function requireInputFile(args: CommandArgs): Promise<string> {
  const input = args.requireString("input");
  let info: Awaited<ReturnType<typeof stat>>;
  try {
    info = await stat(input);
  } catch {
    throw usageError(`--input ${input} does not exist`);
  }
  if (!info.isFile()) throw usageError(`--input ${input} is not a file`);
  if (info.size === 0) throw usageError(`--input ${input} is empty`);
  return input;
}

async function uploadArtifact(
  context: CommandContext,
  grant: ArtifactGrant,
  input: string,
  digestHex: string,
): Promise<void> {
  const body = Readable.toWeb(createReadStream(input));
  await artifactRequest(context, grant, {
    method: "PUT",
    headers: { "Content-Type": "application/x-ndjson", Digest: `sha-256=${digestHex}` },
    body: body as unknown as BodyInit,
    duplex: "half",
  } as RequestInit);
}

function stdoutSink(context: CommandContext): Writable {
  const decoder = new StringDecoder("utf8");
  return new Writable({
    write(chunk: Buffer, _encoding, callback) {
      context.io.stdout.write(decoder.write(chunk));
      callback();
    },
    final(callback) {
      const rest = decoder.end();
      if (rest !== "") context.io.stdout.write(rest);
      callback();
    },
  });
}

function verifyDigest(grant: ArtifactGrant, response: Response, actualHex: string): void {
  const mismatch = (what: string) =>
    new CliError(
      `the downloaded artifact does not match the ${what}; do not use it`,
      EXIT.api,
      "CLI_ARTIFACT_DIGEST_MISMATCH",
    );
  if (grant.digest !== null && grant.digest !== `sha256:${actualHex}`) {
    throw mismatch("grant's digest");
  }
  const header = response.headers.get("digest");
  if (header !== null && header !== `sha-256=${actualHex}`) {
    throw mismatch("response Digest header");
  }
}

/**
 * Streams the export to `output` (or stdout for `-`) while hashing it. A file
 * is written under a `.part` name and only renamed once the digest matches, so
 * the final path never holds a partial or mismatched artifact.
 */
async function downloadArtifact(
  context: CommandContext,
  grant: ArtifactGrant,
  output: string,
): Promise<{ readonly bytes: number; readonly digest: string }> {
  const response = await artifactRequest(context, grant, {
    method: "GET",
    headers: { Accept: "application/x-ndjson" },
  });
  if (response.body === null) {
    throw new CliError("the artifact download had no body", EXIT.api, "CLI_ARTIFACT_EMPTY");
  }
  const hash = createHash("sha256");
  let bytes = 0;
  const counter = new Transform({
    transform(chunk: Buffer, _encoding, callback) {
      hash.update(chunk);
      bytes += chunk.length;
      callback(null, chunk);
    },
  });
  const source = Readable.fromWeb(response.body as unknown as WebReadableStream<Uint8Array>);
  const partial = output === "-" ? undefined : `${output}.part`;
  try {
    await pipeline(
      source,
      counter,
      partial === undefined ? stdoutSink(context) : createWriteStream(partial),
    );
    const digestHex = hash.digest("hex");
    verifyDigest(grant, response, digestHex);
    if (partial !== undefined) await rename(partial, output);
    return { bytes, digest: `sha256:${digestHex}` };
  } catch (error) {
    if (partial !== undefined) await unlink(partial).catch(() => undefined);
    throw error;
  }
}

// ---- data jobs ----------------------------------------------------------------

async function listJobs(context: CommandContext, args: CommandArgs): Promise<void> {
  const tenant = tenantFrom(context, args);
  const client = await context.management();
  const jobs = await client.listDataJobs(tenant.projectId, tenant.environmentId);
  context.out(jobs, { columns: JOB_COLUMNS });
}

async function getJob(context: CommandContext, args: CommandArgs): Promise<void> {
  const tenant = tenantFrom(context, args);
  const jobId = args.requirePositional(0, "job-id");
  const client = await context.management();
  let job = await client.getDataJob(tenant.projectId, tenant.environmentId, jobId);
  if (context.globals.wait) job = await settle(context, client, tenant, job);
  context.out(job);
}

async function cancelJob(context: CommandContext, args: CommandArgs): Promise<void> {
  const tenant = tenantFrom(context, args);
  const jobId = args.requirePositional(0, "job-id");
  const client = await context.management();
  let job = await client.cancelDataJob(tenant.projectId, tenant.environmentId, jobId);
  if (context.globals.wait) job = await settle(context, client, tenant, job);
  context.out(job);
}

async function resumeJob(
  client: MakoManagementClient,
  tenant: Tenant,
  jobId: string,
  kind: DataJob["kind"],
): Promise<DataJob> {
  const job = await client.getDataJob(tenant.projectId, tenant.environmentId, jobId);
  if (job.kind !== kind) throw usageError(`job ${jobId} is an ${job.kind} job, not an ${kind}`);
  return job;
}

// ---- export ------------------------------------------------------------------

async function exportData(context: CommandContext, args: CommandArgs): Promise<void> {
  const tenant = tenantFrom(context, args);
  const output = args.requireString("output");
  if (output === "-" && context.json) {
    throw usageError("--output - writes the artifact to stdout and cannot be combined with --json");
  }
  const client = await context.management();
  const existing = args.string("job");
  let job: DataJob;
  if (existing !== undefined) {
    job = await resumeJob(client, tenant, existing, "export");
  } else {
    const collectionId = args.requireString("collection");
    job = await client.createDataJob(
      tenant.projectId,
      tenant.environmentId,
      { kind: "export", collectionId, conflictStrategy: null },
      context.idempotencyKey(),
    );
    context.info(`job ${job.jobId}`);
    context.info(
      `resume with: ${CLI_NAME} data export --job ${job.jobId} --output ${output} ${tenantFlags(tenant)}`,
    );
  }
  job = await settle(context, client, tenant, job);
  if (job.state !== "succeeded") throw jobFailed(job);
  const grant = await client.createDataJobDownloadGrant(
    tenant.projectId,
    tenant.environmentId,
    job.jobId,
  );
  const { bytes, digest } = await downloadArtifact(context, grant, output);
  context.info(
    `artifact for job ${job.jobId} written to ${output === "-" ? "stdout" : output} (${bytes} bytes, ${digest})`,
  );
  // With `-` the artifact itself is the stdout output; the job went to stderr.
  if (output === "-") return;
  if (context.json) context.out({ job, output, bytes, digest });
  else context.out(job);
}

// ---- import ------------------------------------------------------------------

function strategyFrom(args: CommandArgs): ConflictStrategy {
  const value = args.requireString("strategy");
  const strategy = CONFLICT_STRATEGIES.find((candidate) => candidate === value);
  if (strategy === undefined) {
    throw usageError(`--strategy must be one of ${CONFLICT_STRATEGIES.join(", ")}`);
  }
  return strategy;
}

function schemaVersionFrom(args: CommandArgs): number {
  const schemaVersion = args.integer("schema-version");
  if (schemaVersion === undefined || schemaVersion < 1) {
    throw usageError(
      "--schema-version <n> (the active schema version the rows target) is required",
    );
  }
  return schemaVersion;
}

function printDryRun(context: CommandContext, job: DataJob): void {
  const manifest = job.manifest;
  const progress = job.progress;
  context.info(
    `dry run for import job ${job.jobId} (collection ${job.collectionId}, strategy ${job.conflictStrategy ?? "—"}):`,
  );
  if (manifest !== null) {
    context.info(
      `  schema version ${manifest.schemaVersion}, ${manifest.rowCount} rows, ${manifest.byteCount} bytes, manifest ${manifest.digest}`,
    );
  }
  context.info(
    `  processed ${progress.processed}, committed ${progress.committed}, failed ${progress.failed}, skipped ${progress.skipped}`,
  );
  for (const error of job.errors.slice(0, 5)) context.info(`  error: ${error}`);
  if (job.errors.length > 5) context.info(`  ... ${job.errors.length - 5} more errors`);
  context.info(
    "Confirming starts the import; cancelling it later stops future rows and does not roll back committed rows.",
  );
}

async function importData(context: CommandContext, args: CommandArgs): Promise<void> {
  const tenant = tenantFrom(context, args);
  const client = await context.management();
  const existing = args.string("job");
  let job: DataJob;
  if (existing !== undefined) {
    job = await resumeJob(client, tenant, existing, "import");
  } else {
    const collectionId = args.requireString("collection");
    const conflictStrategy = strategyFrom(args);
    // What the upload and the dry run need is checked before a job exists: a
    // job created and then refused for a missing flag was left behind,
    // awaiting an upload that never came.
    await requireInputFile(args);
    const schemaVersion = schemaVersionFrom(args);
    // Rows for another schema version are refused at the dry run, after the
    // upload; checked here, nothing is created for them.
    const collection = await client.getCollection(
      tenant.projectId,
      tenant.environmentId,
      collectionId,
    );
    if (collection.schemaVersion !== schemaVersion) {
      throw usageError(
        `collection ${collectionId} is at schema version ${collection.schemaVersion}; --schema-version ${schemaVersion} does not match it`,
      );
    }
    job = await client.createDataJob(
      tenant.projectId,
      tenant.environmentId,
      { kind: "import", collectionId, conflictStrategy },
      context.idempotencyKey(),
    );
    context.info(`job ${job.jobId}`);
    context.info(
      `resume with: ${CLI_NAME} data import --job ${job.jobId} --input <path> --schema-version <n> ${tenantFlags(tenant)}`,
    );
  }

  if (job.state === "awaiting_upload" || job.state === "dry_run") {
    const input = await requireInputFile(args);
    const schemaVersion = schemaVersionFrom(args);
    const { digest, bytes } = await sha256File(input);
    if (job.state === "awaiting_upload") {
      const grant = await client.createDataJobUploadGrant(
        tenant.projectId,
        tenant.environmentId,
        job.jobId,
      );
      await uploadArtifact(context, grant, input, digest);
      context.info(`uploaded ${bytes} bytes (sha256:${digest}) for job ${job.jobId}`);
    }
    job = await client.dryRunDataJobImport(tenant.projectId, tenant.environmentId, job.jobId, {
      uploadDigest: `sha256:${digest}`,
      schemaVersion,
    });
  }

  if (job.state === "awaiting_confirmation") {
    printDryRun(context, job);
    if (args.boolean("dry-run-only")) {
      context.info(
        `confirm later with: ${CLI_NAME} data import --job ${job.jobId} --yes ${tenantFlags(tenant)}`,
      );
      context.out(job);
      return;
    }
    const manifestDigest = job.manifest?.digest;
    if (manifestDigest === undefined) {
      throw new CliError(
        `the dry run for job ${job.jobId} returned no manifest digest; nothing was confirmed`,
        EXIT.api,
        "CLI_JOB_NO_MANIFEST",
      );
    }
    // Confirmation is asked here, after the dry run is on screen, rather than
    // by the static destructive marker that would run before anything is known.
    await context.confirmDestructive("confirm import", job.jobId);
    job = await client.confirmDataJob(tenant.projectId, tenant.environmentId, job.jobId, {
      expectedManifestDigest: manifestDigest,
      acknowledgePartialImportCancellation: true,
    });
    context.info(`job ${job.jobId} confirmed: ${job.state}`);
  }

  if (context.globals.wait) job = await settle(context, client, tenant, job);
  if (job.state === "failed" || job.state === "cancelled" || job.state === "expired") {
    throw jobFailed(job);
  }
  context.out(job);
}

export const dataCommands: readonly Command[] = [
  {
    path: ["data", "jobs", "list"],
    summary: "List import and export jobs of an environment",
    operations: ["listDataJobs"],
    options: TENANT_OPTIONS,
    run: listJobs,
  },
  {
    path: ["data", "jobs", "get"],
    summary: "Show one data job; --wait polls it to a terminal state",
    operations: ["getDataJob"],
    positionals: [JOB_POSITIONAL],
    options: TENANT_OPTIONS,
    run: getJob,
  },
  {
    path: ["data", "jobs", "cancel"],
    summary: "Cancel a queued or running data job (committed rows are kept)",
    operations: ["cancelDataJob", "getDataJob"],
    positionals: [JOB_POSITIONAL],
    options: TENANT_OPTIONS,
    destructive: { action: "cancel job", resource: (args) => args.requirePositional(0, "job-id") },
    run: cancelJob,
  },
  {
    path: ["data", "export"],
    summary: "Export a collection snapshot as JSON Lines: create the job, wait, download",
    operations: [
      "createDataJob",
      "getDataJob",
      "createDataJobDownloadGrant",
      "downloadDataJobArtifact",
    ],
    options: {
      ...TENANT_OPTIONS,
      ...COLLECTION_OPTION,
      ...JOB_OPTION,
      output: {
        type: "string",
        description: "Write the artifact here, or - for stdout",
        placeholder: "<path|->",
        required: true,
      },
    },
    run: exportData,
  },
  {
    path: ["data", "import"],
    summary: "Import JSON Lines: upload with its digest, dry-run, then confirm",
    operations: [
      "createDataJob",
      "getDataJob",
      "createDataJobUploadGrant",
      "uploadDataJobArtifact",
      "dryRunDataJobImport",
      "confirmDataJob",
    ],
    options: {
      ...TENANT_OPTIONS,
      ...COLLECTION_OPTION,
      ...JOB_OPTION,
      input: {
        type: "string",
        description: "JSON Lines file to upload (one document per line)",
        placeholder: "<path>",
      },
      strategy: {
        type: "string",
        description: `Conflict strategy: ${CONFLICT_STRATEGIES.join(", ")}`,
        placeholder: "<strategy>",
      },
      "schema-version": {
        type: "string",
        description: "Active schema version the rows conform to",
        placeholder: "<n>",
      },
      "dry-run-only": {
        type: "boolean",
        description: "Stop after the dry run without confirming",
      },
    },
    run: importData,
  },
];
