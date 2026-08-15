declare const projectIdBrand: unique symbol;
declare const environmentIdBrand: unique symbol;
declare const collectionIdBrand: unique symbol;

export type ProjectId = string & { readonly [projectIdBrand]: true };
export type EnvironmentId = string & { readonly [environmentIdBrand]: true };
export type CollectionId = string & { readonly [collectionIdBrand]: true };

export interface TenantScope {
  readonly projectId: ProjectId;
  readonly environmentId: EnvironmentId;
}

export interface CollectionScope extends TenantScope {
  readonly collectionId: CollectionId;
}

const PROJECT_ID = /^prj_[A-Za-z0-9_-]{8,64}$/;
const ENVIRONMENT_ID = /^env_[A-Za-z0-9_-]{8,64}$/;
const COLLECTION_ID = /^[a-z][a-z0-9_-]{0,62}$/;

export class ScopeValidationError extends Error {
  public constructor(
    public readonly code:
      | "missing_project"
      | "missing_environment"
      | "invalid_project"
      | "invalid_environment"
      | "invalid_collection"
      | "tenant_mismatch",
  ) {
    super(code.replaceAll("_", " "));
    this.name = "ScopeValidationError";
  }
}

export function parseProjectId(value: string): ProjectId {
  if (!PROJECT_ID.test(value)) {
    throw new ScopeValidationError("invalid_project");
  }
  return value as ProjectId;
}

export function parseEnvironmentId(value: string): EnvironmentId {
  if (!ENVIRONMENT_ID.test(value)) {
    throw new ScopeValidationError("invalid_environment");
  }
  return value as EnvironmentId;
}

export function parseCollectionId(value: string): CollectionId {
  if (!COLLECTION_ID.test(value)) {
    throw new ScopeValidationError("invalid_collection");
  }
  return value as CollectionId;
}

export function requireTenantScope(
  projectId: string | null | undefined,
  environmentId: string | null | undefined,
): TenantScope {
  if (projectId === null || projectId === undefined) {
    throw new ScopeValidationError("missing_project");
  }
  if (environmentId === null || environmentId === undefined) {
    throw new ScopeValidationError("missing_environment");
  }
  return {
    projectId: parseProjectId(projectId),
    environmentId: parseEnvironmentId(environmentId),
  };
}

export function requireCollectionScope(
  projectId: string | null | undefined,
  environmentId: string | null | undefined,
  collectionId: string,
): CollectionScope {
  return {
    ...requireTenantScope(projectId, environmentId),
    collectionId: parseCollectionId(collectionId),
  };
}

export function assertSameTenant(requested: TenantScope, trusted: TenantScope): void {
  if (
    requested.projectId !== trusted.projectId ||
    requested.environmentId !== trusted.environmentId
  ) {
    throw new ScopeValidationError("tenant_mismatch");
  }
}
