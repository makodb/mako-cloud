import {
  parseCollectionId,
  parseEnvironmentId,
  parseProjectId,
  type CollectionId,
  type EnvironmentId,
  type ProjectId,
} from "./wire.js";

export const SUPPORTED_RXDB_MAJOR = 17 as const;
export const SUPPORTED_RXDB_RANGE = ">=17.0.0 <18.0.0" as const;

export type MakoRxdbRuntime = "browser" | "node";

export interface MakoRxdbClientConfig {
  readonly endpoint: string;
  readonly projectId: string;
  readonly environmentId: string;
  readonly collectionId: string;
  readonly schemaVersion: number;
  readonly publicProjectKey: string;
  readonly rxdbVersion: string;
  readonly runtime: MakoRxdbRuntime;
  readonly pullBatchSize?: number;
  readonly pushBatchSize?: number;
}

export interface NormalizedMakoRxdbClientConfig {
  readonly endpoint: URL;
  readonly projectId: ProjectId;
  readonly environmentId: EnvironmentId;
  readonly collectionId: CollectionId;
  readonly schemaVersion: number;
  readonly publicProjectKey: string;
  readonly rxdbVersion: string;
  readonly runtime: MakoRxdbRuntime;
  readonly pullBatchSize: number;
  readonly pushBatchSize: number;
}

export class MakoRxdbConfigurationError extends Error {
  override readonly name: string = "MakoRxdbConfigurationError";
}

export class UnsupportedRxdbVersionError extends MakoRxdbConfigurationError {
  override readonly name: string = "UnsupportedRxdbVersionError";
  readonly detectedVersion: string;
  readonly supportedRange = SUPPORTED_RXDB_RANGE;

  constructor(detectedVersion: string) {
    super(`RxDB ${detectedVersion} is unsupported; expected ${SUPPORTED_RXDB_RANGE}`);
    this.detectedVersion = detectedVersion;
  }
}

export function assertSupportedRxdbVersion(version: string): void {
  const match = /^(\d+)\.(\d+)\.(\d+)$/.exec(version);
  if (match?.[1] !== String(SUPPORTED_RXDB_MAJOR)) {
    throw new UnsupportedRxdbVersionError(version);
  }
}

export function normalizeMakoRxdbConfig(
  config: MakoRxdbClientConfig,
): NormalizedMakoRxdbClientConfig {
  assertSupportedRxdbVersion(config.rxdbVersion);
  const endpoint = parseEndpoint(config.endpoint);
  if (!Number.isSafeInteger(config.schemaVersion) || config.schemaVersion < 1) {
    throw new MakoRxdbConfigurationError("schemaVersion must be a positive safe integer");
  }
  if (
    !config.publicProjectKey.startsWith("mako_pk.") ||
    config.publicProjectKey.length > 512 ||
    hasControlCharacters(config.publicProjectKey)
  ) {
    throw new MakoRxdbConfigurationError("publicProjectKey is invalid");
  }
  const pullBatchSize = config.pullBatchSize ?? 100;
  if (!Number.isSafeInteger(pullBatchSize) || pullBatchSize < 1 || pullBatchSize > 1_000) {
    throw new MakoRxdbConfigurationError("pullBatchSize must be between 1 and 1000");
  }
  const pushBatchSize = config.pushBatchSize ?? 100;
  if (!Number.isSafeInteger(pushBatchSize) || pushBatchSize < 1 || pushBatchSize > 1_000) {
    throw new MakoRxdbConfigurationError("pushBatchSize must be between 1 and 1000");
  }
  return {
    endpoint,
    projectId: parseProjectId(config.projectId),
    environmentId: parseEnvironmentId(config.environmentId),
    collectionId: parseCollectionId(config.collectionId),
    schemaVersion: config.schemaVersion,
    publicProjectKey: config.publicProjectKey,
    rxdbVersion: config.rxdbVersion,
    runtime: config.runtime,
    pullBatchSize,
    pushBatchSize,
  };
}

function hasControlCharacters(value: string): boolean {
  return Array.from(value).some((character) => {
    const codePoint = character.codePointAt(0);
    return codePoint !== undefined && (codePoint <= 0x1f || codePoint === 0x7f);
  });
}

function parseEndpoint(value: string): URL {
  let endpoint: URL;
  try {
    endpoint = new URL(value);
  } catch {
    throw new MakoRxdbConfigurationError("endpoint must be an absolute URL");
  }
  const localHttp =
    endpoint.protocol === "http:" &&
    (endpoint.hostname === "localhost" || endpoint.hostname === "127.0.0.1");
  if (endpoint.protocol !== "https:" && !localHttp) {
    throw new MakoRxdbConfigurationError("endpoint must use HTTPS outside local development");
  }
  endpoint.pathname = endpoint.pathname.replace(/\/+$/u, "");
  endpoint.search = "";
  endpoint.hash = "";
  return endpoint;
}
