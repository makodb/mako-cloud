/** Typed Mako Cloud management API client. */
import {
  createMakoApiClient,
  isApiErrorEnvelope,
  type ApiError,
  type MakoApiClient,
  type components,
} from "@mako-cloud/api-types";

export const PACKAGE_NAME = "@mako-cloud/management-sdk" as const;

export const DEVELOPER_AUTH_OPERATIONS = [
  "registerDeveloper",
  "verifyDeveloperEmail",
  "resendDeveloperVerification",
  "createDeveloperSession",
  "refreshDeveloperSession",
  "deleteDeveloperSession",
  "requestDeveloperPasswordRecovery",
  "completeDeveloperPasswordRecovery",
  "getDeveloperWaitListStatus",
  "verifyCurrentDeveloperPassword",
] as const;

export const MANAGEMENT_OPERATIONS = [
  "listOrganizations",
  "createOrganization",
  "getOrganization",
  "updateOrganization",
  "requestOrganizationDeletion",
  "restoreOrganization",
  "createOrganizationInvitation",
  "acceptOrganizationInvitation",
  "listOrganizationMembers",
  "updateOrganizationMember",
  "removeOrganizationMember",
  "listAutomationTokens",
  "createAutomationToken",
  "revokeAutomationToken",
  "rotateAutomationToken",
  "listProjects",
  "createProject",
  "getProject",
  "requestProjectDeletion",
  "suspendProject",
  "restoreProject",
  "listEnvironments",
  "createEnvironment",
  "getEnvironment",
  "requestEnvironmentDeletion",
  "suspendEnvironment",
  "restoreEnvironment",
  "listCollections",
  "createCollection",
  "getCollection",
  "publishCollectionSchema",
  "createSchemaMigration",
  "getSchemaMigration",
  "updateSchemaMigration",
  "listCollectionIndexes",
  "createCollectionIndex",
  "getCollectionIndex",
  "deleteCollectionIndex",
  "getActiveCollectionPolicy",
  "createCollectionPolicyDraft",
  "getCollectionPolicy",
  "validateCollectionPolicy",
  "testCollectionPolicy",
  "activateCollectionPolicy",
  "rollbackCollectionPolicy",
  "searchApplicationUsers",
  "createApplicationUser",
  "inviteApplicationUser",
  "getApplicationUser",
  "updateApplicationUserMetadata",
  "deleteApplicationUser",
  "disableApplicationUser",
  "restoreApplicationUser",
  "revokeApplicationUserSessions",
  "revokeApplicationUserSession",
  "createPublicProjectKey",
  "createServiceCredential",
  "getProjectCredential",
  "retireProjectCredential",
  "rotateProjectCredential",
  "initializeJwtSigningKey",
  "listJwtSigningKeys",
  "rotateJwtSigningKey",
  "createFunctionSecret",
  "getFunctionSecret",
  "retireFunctionSecret",
  "rotateFunctionSecret",
  "uploadFunctionBundle",
  "listFunctions",
  "createFunction",
  "getFunction",
  "updateFunctionConfiguration",
  "deleteFunction",
  "listFunctionDeployments",
  "createFunctionDeployment",
  "getFunctionDeployment",
  "deleteFunctionDeployment",
  "checkFunctionDeploymentHealth",
  "promoteFunctionDeployment",
  "rollbackFunctionDeployment",
  "testFunctionInvocation",
  "queryFunctionLogs",
  "queryProjectUsage",
  "queryProjectQuotas",
  "queryProjectHealth",
  "queryReplicationErrors",
  "queryAuthenticationEvents",
  "queryFunctionMetrics",
  "queryProjectLogs",
  "queryIndexStateEvents",
  "queryAuditEvents",
  "issueExplorerGrant",
  "revokeExplorerGrant",
  "explorerGetDocument",
  "explorerBrowseDocuments",
  "explorerPlanQuery",
  "explorerQueryDocuments",
  "explorerDocumentHistory",
  "explorerSimulateMutation",
  "explorerMutateDocument",
  "listDataJobs",
  "createDataJob",
  "getDataJob",
  "cancelDataJob",
  "dryRunDataJobImport",
  "confirmDataJob",
  "createDataJobUploadGrant",
  "createDataJobDownloadGrant",
  "getWorkspaceSummary",
  "getWorkspaceNavigation",
  "getConnectMetadata",
  "checkConnection",
  "getSyncSummary",
  "listDeveloperBackups",
  "listDeveloperRestoreRequests",
  "requestDeveloperRestore",
  "verifyCurrentDeveloperPassword",
] as const;

export const OPERATOR_OPERATIONS = [
  "createOperatorSession",
  "getCurrentOperatorSession",
  "deleteCurrentOperatorSession",
  "verifyCurrentOperatorPassword",
  "getOperatorOverview",
  "listOperatorTenants",
  "getOperatorTenant360",
  "getOperatorInventory",
  "listOperatorAlerts",
  "getOperatorAlert",
  "listOperatorIncidents",
  "getOperatorIncident",
  "createOperatorIncident",
  "updateOperatorIncident",
  "createOperatorRecoveryJob",
  "listOperatorRecoveryJobs",
  "getOperatorRecoveryJob",
  "advanceOperatorRecoveryJob",
  "listOperatorActivity",
  "createOperatorActivityExport",
  "getOperatorActivityExport",
  "processOperatorActivityExport",
  "rebuildOperatorProjection",
  "listOperatorProvisioningWorkflows",
  "listOperatorQuotaOverrides",
  "replaceOperatorQuotaOverride",
  "revokeOperatorQuotaOverride",
  "listOperatorAbuseResponses",
  "restoreOperatorAbuseResponse",
  "listOperatorSupportSessions",
  "listCurrentOperatorSupportSessions",
  "getOperatorSecurityInventory",
  "revokeOperatorIdentitySessions",
  "planOperatorEntitlementChange",
  "applyOperatorEntitlementChange",
  "getOperatorProject",
  "repairOperatorProvisioning",
  "createOperatorQuotaOverride",
  "createOperatorAbuseResponse",
  "createSupportSession",
  "revokeSupportSession",
  "listDeveloperWaitList",
  "getDeveloperWaitListApplicant",
  "approveDeveloperWaitListApplicant",
  "rejectDeveloperWaitListApplicant",
] as const;

export type ManagementOperation = (typeof MANAGEMENT_OPERATIONS)[number];
export type Organization = components["schemas"]["Organization"];
export type OrganizationRole = components["schemas"]["OrganizationRole"];
export type OrganizationMembership = components["schemas"]["OrganizationMembership"];
export type InvitationIssue = components["schemas"]["InvitationIssue"];
export type AutomationPermission = components["schemas"]["AutomationPermission"];
export type AutomationScope = components["schemas"]["AutomationScope"];
export type AutomationToken = components["schemas"]["AutomationToken"];
export type AutomationTokenIssue = components["schemas"]["AutomationTokenIssue"];
export type Project = components["schemas"]["Project"];
export type Environment = components["schemas"]["Environment"];
export type Collection = components["schemas"]["Collection"];
export type CreateCollectionRequest = components["schemas"]["CreateCollectionRequest"];
export type PublishCollectionSchemaRequest =
  components["schemas"]["PublishCollectionSchemaRequest"];
export type SchemaPublicationResult = components["schemas"]["SchemaPublicationResult"];
export type CreateSchemaMigrationRequest = components["schemas"]["CreateSchemaMigrationRequest"];
export type SchemaMigration = components["schemas"]["SchemaMigration"];
export type SchemaMigrationState = components["schemas"]["SchemaMigrationState"];
export type CreateCollectionIndexRequest = components["schemas"]["CreateCollectionIndexRequest"];
export type CollectionIndex = components["schemas"]["CollectionIndex"];
export type PolicySet = components["schemas"]["PolicySet"];
export type ActivePolicy = components["schemas"]["ActivePolicy"];
export type PolicyValidation = components["schemas"]["PolicyValidation"];
export type PolicyExample = components["schemas"]["PolicyExample"];
export type PolicyExampleResult = components["schemas"]["PolicyExampleResult"];
export type CreatePolicyDraftRequest = components["schemas"]["CreatePolicyDraftRequest"];
export type ApplicationUserSummary = components["schemas"]["ApplicationUserSummary"];
export type ApplicationUserView = components["schemas"]["ApplicationUserView"];
export type AdminCreateUserRequest = components["schemas"]["AdminCreateUserRequest"];
export type AdminUpdateUserMetadataRequest =
  components["schemas"]["AdminUpdateUserMetadataRequest"];
export type ProjectCredential = components["schemas"]["ProjectCredential"];
export type ProjectCredentialIssue = components["schemas"]["ProjectCredentialIssue"];
export type ServiceCredentialScope = components["schemas"]["ServiceCredentialScope"];
export type JwtSigningKey = components["schemas"]["JwtSigningKey"];
export type FunctionSecret = components["schemas"]["FunctionSecret"];
export type FunctionSecretIssue = components["schemas"]["FunctionSecretIssue"];
export type FunctionBundleUploadRequest = components["schemas"]["FunctionBundleUploadRequest"];
export type FunctionBundleArtifact = components["schemas"]["FunctionBundleArtifact"];
export type FunctionBundleUploadResult = components["schemas"]["FunctionBundleUploadResult"];
export type FunctionConfiguration = components["schemas"]["FunctionConfiguration"];
export type CreateFunctionRequest = components["schemas"]["CreateFunctionRequest"];
export type Function = components["schemas"]["Function"];
export type FunctionDeploymentRequest = components["schemas"]["FunctionDeploymentRequest"];
export type FunctionDeployment = components["schemas"]["FunctionDeployment"];
export type FunctionTestRequest = components["schemas"]["FunctionTestRequest"];
export type FunctionTestResponse = components["schemas"]["FunctionTestResponse"];
export type FunctionLogPage = components["schemas"]["FunctionLogPage"];
export type ObservabilityPage = components["schemas"]["ObservabilityPage"];
export type ObservabilityRecord = components["schemas"]["ObservabilityRecord"];
export type ObservabilityPayload = components["schemas"]["ObservabilityPayload"];
export type ExplorerGrantRequest = components["schemas"]["ExplorerGrantRequest"];
export type ExplorerGrant = components["schemas"]["ExplorerGrant"];
export type ExplorerGrantRevocation = components["schemas"]["ExplorerGrantRevocation"];
export type ExplorerDocument = components["schemas"]["ExplorerDocument"];
export type ExplorerDocumentPage = components["schemas"]["ExplorerDocumentPage"];
export type ExplorerPageRequest = components["schemas"]["ExplorerPageRequest"];
export type ExplorerQueryRequest = components["schemas"]["ExplorerQueryRequest"];
export type ExplorerQueryPlan = components["schemas"]["ExplorerQueryPlan"];
export type ExplorerMutationRequest = components["schemas"]["ExplorerMutationRequest"];
export type ExplorerMutationResult = components["schemas"]["ExplorerMutationResult"];
export type ExplorerSimulation = components["schemas"]["ExplorerSimulation"];
export type ExplorerRevision = components["schemas"]["ExplorerRevision"];
export type DataJob = components["schemas"]["DataJob"];
export type DataJobCreateRequest = components["schemas"]["DataJobCreateRequest"];
export type DataJobDryRunRequest = components["schemas"]["DataJobDryRunRequest"];
export type DataJobConfirmationRequest = components["schemas"]["DataJobConfirmationRequest"];
export type ArtifactGrant = components["schemas"]["ArtifactGrant"];
export type WorkspaceSummary = components["schemas"]["WorkspaceSummary"];
export type WorkspaceDestination = components["schemas"]["WorkspaceDestination"];
export type ConnectMetadata = components["schemas"]["ConnectMetadata"];
export type ConnectionCheck = components["schemas"]["ConnectionCheck"];
export type ConnectionCheckRequest = components["schemas"]["ConnectionCheckRequest"];
export type SyncSummary = components["schemas"]["SyncSummary"];
export type DeveloperBackup = components["schemas"]["DeveloperBackup"];
export type DeveloperRestore = components["schemas"]["DeveloperRestore"];
export type DeveloperRestoreRequest = components["schemas"]["DeveloperRestoreRequest"];
export type DeveloperStepUpGrant = components["schemas"]["DeveloperStepUpGrant"];
export type OperatorProjectView = components["schemas"]["OperatorProjectView"];
export type ProvisioningWorkflow = components["schemas"]["ProvisioningWorkflow"];
export type CreateQuotaOverrideRequest = components["schemas"]["CreateQuotaOverrideRequest"];
export type QuotaOverride = components["schemas"]["QuotaOverride"];
export type CreateAbuseResponseRequest = components["schemas"]["CreateAbuseResponseRequest"];
export type AbuseResponse = components["schemas"]["AbuseResponse"];
export type SupportPermission = components["schemas"]["SupportPermission"];
export type SupportSession = components["schemas"]["SupportSession"];
export type DeveloperRegistrationRequest = components["schemas"]["DeveloperRegistrationRequest"];
export type DeveloperSession = components["schemas"]["DeveloperSessionResponse"];
export type DeveloperWaitListStatus = components["schemas"]["DeveloperWaitListStatus"];
export type DeveloperApplicant = components["schemas"]["DeveloperApplicant"];
export type DeveloperApplicantPage = components["schemas"]["DeveloperApplicantPage"];
export type DeveloperGenericAccepted = components["schemas"]["DeveloperGenericAccepted"];
export type OperatorPermission = components["schemas"]["OperatorPermission"];
export type OperatorSession = components["schemas"]["OperatorSessionResponse"];
export type OperatorOverview = components["schemas"]["OperatorOverview"];
export type OperatorTenantPage = components["schemas"]["OperatorTenantPage"];
export type OperatorTenant360 = components["schemas"]["OperatorTenant360"];
export type OperatorTenantSummary = components["schemas"]["OperatorTenantSummary"];
export type OperatorInventoryKind = components["schemas"]["OperatorInventoryKind"];
export type OperatorReadSection = components["schemas"]["OperatorReadSection"];
export type OperatorIncident = components["schemas"]["OperatorIncident"];
export type OperatorIncidentPage = components["schemas"]["OperatorIncidentPage"];
export type OperatorAlert = components["schemas"]["OperatorAlert"];
export type OperatorAlertPage = components["schemas"]["OperatorAlertPage"];
export type CreateOperatorIncidentRequest = components["schemas"]["CreateOperatorIncidentRequest"];
export type UpdateOperatorIncidentRequest = components["schemas"]["UpdateOperatorIncidentRequest"];
export type OperatorRecoveryJob = components["schemas"]["OperatorRecoveryJob"];
export type OperatorRecoveryJobPage = components["schemas"]["OperatorRecoveryJobPage"];
export type CreateOperatorRecoveryJobRequest =
  components["schemas"]["CreateOperatorRecoveryJobRequest"];
export type AdvanceOperatorRecoveryJobRequest =
  components["schemas"]["AdvanceOperatorRecoveryJobRequest"];
export type OperatorActivityPage = components["schemas"]["OperatorActivityPage"];
export type OperatorActivityExport = components["schemas"]["OperatorActivityExport"];
export type CreateOperatorActivityExportRequest =
  components["schemas"]["CreateOperatorActivityExportRequest"];
export type OperatorProjectionEvidence = components["schemas"]["OperatorProjectionEvidence"];
export type OperatorSecurityInventory = components["schemas"]["OperatorSecurityInventory"];
export type OperatorEntitlementChangeInput =
  components["schemas"]["OperatorEntitlementChangeInput"];
export type OperatorEntitlementChangePlan = components["schemas"]["OperatorEntitlementChangePlan"];
export type OperatorEntitlementChangeResult =
  components["schemas"]["OperatorEntitlementChangeResult"];

export type OperatorOperation = (typeof OPERATOR_OPERATIONS)[number];

export interface ObservabilityQuery {
  readonly cursor?: string;
  readonly from?: string;
  readonly until?: string;
  readonly limit?: number;
}

export type ManagementCredential =
  | {
      readonly kind: "developer_session";
      readonly accessToken: TokenProvider;
    }
  | {
      readonly kind: "automation_token";
      readonly accessToken: TokenProvider;
    };

export type TokenProvider = string | (() => Promise<string> | string);

export interface ManagementClientOptions {
  readonly endpoint: string;
  readonly credential: ManagementCredential;
  readonly fetch?: typeof globalThis.fetch;
}

export interface OperatorClientOptions {
  readonly endpoint: string;
  readonly fetch?: typeof globalThis.fetch;
}

export interface DeveloperAuthClientOptions {
  readonly endpoint: string;
  readonly fetch?: typeof globalThis.fetch;
}

export class ManagementApiError extends Error {
  override readonly name = "ManagementApiError";
  readonly code: ApiError["code"];
  readonly requestId: string;
  readonly retry: ApiError["retry"];
  readonly status: number;

  constructor(error: ApiError, status: number) {
    super(error.message);
    this.code = error.code;
    this.requestId = error.requestId;
    this.retry = error.retry;
    this.status = status;
  }
}

export class MakoManagementClient {
  readonly #client: MakoApiClient;
  readonly #credential: ManagementCredential;

  constructor(options: ManagementClientOptions) {
    this.#credential = options.credential;
    const baseUrl = normalizeEndpoint(options.endpoint);
    const requestFetch = options.fetch;
    this.#client =
      requestFetch === undefined
        ? createMakoApiClient({ baseUrl })
        : createMakoApiClient({ baseUrl, fetch: (request) => requestFetch(request) });
    this.#client.use({
      onRequest: async ({ request }) => {
        request.headers.set("Authorization", `Bearer ${await this.#accessToken()}`);
        request.headers.set("Accept", "application/json");
        return request;
      },
    });
  }

  async listOrganizations(): Promise<Organization[]> {
    const result = await this.#client.GET("/v1/organizations");
    return unwrap(result).items;
  }

  async createOrganization(name: string): Promise<Organization> {
    return unwrap(
      await this.#client.POST("/v1/organizations", {
        body: { name },
      }),
    );
  }

  async getOrganization(organizationId: string): Promise<Organization> {
    return unwrap(
      await this.#client.GET("/v1/organizations/{organizationId}", {
        params: { path: { organizationId } },
      }),
    );
  }

  async updateOrganization(organizationId: string, name: string): Promise<Organization> {
    return unwrap(
      await this.#client.PATCH("/v1/organizations/{organizationId}", {
        params: { path: { organizationId } },
        body: { name },
      }),
    );
  }

  async requestOrganizationDeletion(
    organizationId: string,
    confirmation: string,
  ): Promise<Organization> {
    return unwrap(
      await this.#client.DELETE("/v1/organizations/{organizationId}", {
        params: { path: { organizationId }, header: { confirmation } },
      }),
    );
  }

  async restoreOrganization(organizationId: string, idempotencyKey: string): Promise<Organization> {
    return unwrap(
      await this.#client.POST("/v1/organizations/{organizationId}/actions/restore", {
        params: {
          path: { organizationId },
          header: { "Idempotency-Key": idempotencyKey },
        },
      }),
    );
  }

  async createInvitation(
    organizationId: string,
    input: { readonly email: string; readonly role: OrganizationRole; readonly expiresAt: string },
  ): Promise<InvitationIssue> {
    return unwrap(
      await this.#client.POST("/v1/organizations/{organizationId}/invitations", {
        params: { path: { organizationId } },
        body: input,
      }),
    );
  }

  async acceptInvitation(invitationId: string, token: string): Promise<OrganizationMembership> {
    return unwrap(
      await this.#client.POST("/v1/invitations/{invitationId}/accept", {
        params: { path: { invitationId } },
        body: { token },
      }),
    );
  }

  async listMembers(organizationId: string): Promise<OrganizationMembership[]> {
    const result = await this.#client.GET("/v1/organizations/{organizationId}/members", {
      params: { path: { organizationId } },
    });
    return unwrap(result).items;
  }

  async updateMember(
    organizationId: string,
    developerIdentityId: string,
    role: OrganizationRole,
  ): Promise<OrganizationMembership> {
    return unwrap(
      await this.#client.PATCH("/v1/organizations/{organizationId}/members/{developerIdentityId}", {
        params: { path: { organizationId, developerIdentityId } },
        body: { role },
      }),
    );
  }

  async removeMember(organizationId: string, developerIdentityId: string): Promise<void> {
    expectNoContent(
      await this.#client.DELETE(
        "/v1/organizations/{organizationId}/members/{developerIdentityId}",
        { params: { path: { organizationId, developerIdentityId } } },
      ),
    );
  }

  async listAutomationTokens(organizationId: string): Promise<AutomationToken[]> {
    const result = await this.#client.GET("/v1/organizations/{organizationId}/automation-tokens", {
      params: { path: { organizationId } },
    });
    return unwrap(result).items;
  }

  async createAutomationToken(
    organizationId: string,
    input: {
      readonly name: string;
      readonly scope: AutomationScope;
      readonly expiresAt: string;
    },
  ): Promise<AutomationTokenIssue> {
    return unwrap(
      await this.#client.POST("/v1/organizations/{organizationId}/automation-tokens", {
        params: { path: { organizationId } },
        body: input,
      }),
    );
  }

  async revokeAutomationToken(organizationId: string, automationTokenId: string): Promise<void> {
    expectNoContent(
      await this.#client.DELETE(
        "/v1/organizations/{organizationId}/automation-tokens/{automationTokenId}",
        { params: { path: { organizationId, automationTokenId } } },
      ),
    );
  }

  async rotateAutomationToken(
    organizationId: string,
    automationTokenId: string,
    replacementId: string,
    expiresAt: string,
    idempotencyKey: string,
  ): Promise<AutomationTokenIssue> {
    return unwrap(
      await this.#client.POST(
        "/v1/organizations/{organizationId}/automation-tokens/{automationTokenId}/actions/rotate",
        {
          params: {
            path: { organizationId, automationTokenId },
            header: { "Idempotency-Key": idempotencyKey },
          },
          body: { replacementId, expiresAt },
        },
      ),
    );
  }

  async createProject(
    input: { readonly organizationId: string; readonly name: string; readonly region: string },
    idempotencyKey: string,
  ): Promise<Project> {
    return unwrap(
      await this.#client.POST("/v1/projects", {
        params: { header: { "Idempotency-Key": idempotencyKey } },
        body: input,
      }),
    );
  }

  async listProjects(organizationId: string): Promise<Project[]> {
    const result = await this.#client.GET("/v1/projects", {
      params: { query: { organizationId } },
    });
    return unwrap(result).items;
  }

  async getProject(projectId: string): Promise<Project> {
    return unwrap(
      await this.#client.GET("/v1/projects/{projectId}", {
        params: { path: { projectId } },
      }),
    );
  }

  async requestProjectDeletion(projectId: string, confirmation: string): Promise<Project> {
    return unwrap(
      await this.#client.DELETE("/v1/projects/{projectId}", {
        params: { path: { projectId }, header: { confirmation } },
      }),
    );
  }

  async suspendProject(projectId: string, idempotencyKey: string): Promise<Project> {
    return unwrap(
      await this.#client.POST("/v1/projects/{projectId}/actions/suspend", {
        params: { path: { projectId }, header: { "Idempotency-Key": idempotencyKey } },
      }),
    );
  }

  async restoreProject(projectId: string, idempotencyKey: string): Promise<Project> {
    return unwrap(
      await this.#client.POST("/v1/projects/{projectId}/actions/restore", {
        params: { path: { projectId }, header: { "Idempotency-Key": idempotencyKey } },
      }),
    );
  }

  async listEnvironments(projectId: string): Promise<Environment[]> {
    const result = await this.#client.GET("/v1/projects/{projectId}/environments", {
      params: { path: { projectId } },
    });
    return unwrap(result).items;
  }

  async createEnvironment(
    projectId: string,
    name: string,
    idempotencyKey: string,
  ): Promise<Environment> {
    return unwrap(
      await this.#client.POST("/v1/projects/{projectId}/environments", {
        params: { path: { projectId }, header: { "Idempotency-Key": idempotencyKey } },
        body: { name },
      }),
    );
  }

  async getEnvironment(projectId: string, environmentId: string): Promise<Environment> {
    return unwrap(
      await this.#client.GET("/v1/projects/{projectId}/environments/{environmentId}", {
        params: { path: { projectId, environmentId } },
      }),
    );
  }

  async requestEnvironmentDeletion(
    projectId: string,
    environmentId: string,
    confirmation: string,
  ): Promise<Environment> {
    return unwrap(
      await this.#client.DELETE("/v1/projects/{projectId}/environments/{environmentId}", {
        params: { path: { projectId, environmentId }, header: { confirmation } },
      }),
    );
  }

  async suspendEnvironment(
    projectId: string,
    environmentId: string,
    idempotencyKey: string,
  ): Promise<Environment> {
    return unwrap(
      await this.#client.POST(
        "/v1/projects/{projectId}/environments/{environmentId}/actions/suspend",
        {
          params: {
            path: { projectId, environmentId },
            header: { "Idempotency-Key": idempotencyKey },
          },
        },
      ),
    );
  }

  async restoreEnvironment(
    projectId: string,
    environmentId: string,
    idempotencyKey: string,
  ): Promise<Environment> {
    return unwrap(
      await this.#client.POST(
        "/v1/projects/{projectId}/environments/{environmentId}/actions/restore",
        {
          params: {
            path: { projectId, environmentId },
            header: { "Idempotency-Key": idempotencyKey },
          },
        },
      ),
    );
  }

  async listCollections(projectId: string, environmentId: string): Promise<Collection[]> {
    const result = await this.#client.GET(
      "/v1/projects/{projectId}/environments/{environmentId}/collections",
      { params: { path: { projectId, environmentId } } },
    );
    return unwrap(result).items;
  }

  async createCollection(
    projectId: string,
    environmentId: string,
    input: CreateCollectionRequest,
    idempotencyKey: string,
  ): Promise<Collection> {
    return unwrap(
      await this.#client.POST("/v1/projects/{projectId}/environments/{environmentId}/collections", {
        params: {
          path: { projectId, environmentId },
          header: { "Idempotency-Key": idempotencyKey },
        },
        body: input,
      }),
    );
  }

  async getCollection(
    projectId: string,
    environmentId: string,
    collectionId: string,
  ): Promise<Collection> {
    return unwrap(
      await this.#client.GET(
        "/v1/projects/{projectId}/environments/{environmentId}/collections/{collectionId}",
        { params: { path: { projectId, environmentId, collectionId } } },
      ),
    );
  }

  async publishCollectionSchema(
    projectId: string,
    environmentId: string,
    collectionId: string,
    input: PublishCollectionSchemaRequest,
    idempotencyKey: string,
  ): Promise<SchemaPublicationResult> {
    return unwrap(
      await this.#client.POST(
        "/v1/projects/{projectId}/environments/{environmentId}/collections/{collectionId}/schemas",
        {
          params: {
            path: { projectId, environmentId, collectionId },
            header: { "Idempotency-Key": idempotencyKey },
          },
          body: input,
        },
      ),
    );
  }

  async createSchemaMigration(
    projectId: string,
    environmentId: string,
    collectionId: string,
    input: CreateSchemaMigrationRequest,
    idempotencyKey: string,
  ): Promise<SchemaMigration> {
    return unwrap(
      await this.#client.POST(
        "/v1/projects/{projectId}/environments/{environmentId}/collections/{collectionId}/migrations",
        {
          params: {
            path: { projectId, environmentId, collectionId },
            header: { "Idempotency-Key": idempotencyKey },
          },
          body: input,
        },
      ),
    );
  }

  async getSchemaMigration(
    projectId: string,
    environmentId: string,
    collectionId: string,
    migrationId: string,
  ): Promise<SchemaMigration> {
    return unwrap(
      await this.#client.GET(
        "/v1/projects/{projectId}/environments/{environmentId}/collections/{collectionId}/migrations/{migrationId}",
        { params: { path: { projectId, environmentId, collectionId, migrationId } } },
      ),
    );
  }

  async updateSchemaMigration(
    projectId: string,
    environmentId: string,
    collectionId: string,
    migrationId: string,
    state: SchemaMigrationState,
  ): Promise<SchemaMigration> {
    return unwrap(
      await this.#client.PATCH(
        "/v1/projects/{projectId}/environments/{environmentId}/collections/{collectionId}/migrations/{migrationId}",
        {
          params: { path: { projectId, environmentId, collectionId, migrationId } },
          body: { state },
        },
      ),
    );
  }

  async listCollectionIndexes(
    projectId: string,
    environmentId: string,
    collectionId: string,
  ): Promise<CollectionIndex[]> {
    const result = await this.#client.GET(
      "/v1/projects/{projectId}/environments/{environmentId}/collections/{collectionId}/indexes",
      { params: { path: { projectId, environmentId, collectionId } } },
    );
    return unwrap(result).items;
  }

  async createCollectionIndex(
    projectId: string,
    environmentId: string,
    collectionId: string,
    input: CreateCollectionIndexRequest,
    idempotencyKey: string,
  ): Promise<CollectionIndex> {
    return unwrap(
      await this.#client.POST(
        "/v1/projects/{projectId}/environments/{environmentId}/collections/{collectionId}/indexes",
        {
          params: {
            path: { projectId, environmentId, collectionId },
            header: { "Idempotency-Key": idempotencyKey },
          },
          body: input,
        },
      ),
    );
  }

  async getCollectionIndex(
    projectId: string,
    environmentId: string,
    collectionId: string,
    indexName: string,
    indexVersion: number,
  ): Promise<CollectionIndex> {
    return unwrap(
      await this.#client.GET(
        "/v1/projects/{projectId}/environments/{environmentId}/collections/{collectionId}/indexes/{indexName}/{indexVersion}",
        {
          params: {
            path: { projectId, environmentId, collectionId, indexName, indexVersion },
          },
        },
      ),
    );
  }

  async deleteCollectionIndex(
    projectId: string,
    environmentId: string,
    collectionId: string,
    indexName: string,
    indexVersion: number,
  ): Promise<CollectionIndex> {
    return unwrap(
      await this.#client.DELETE(
        "/v1/projects/{projectId}/environments/{environmentId}/collections/{collectionId}/indexes/{indexName}/{indexVersion}",
        {
          params: {
            path: { projectId, environmentId, collectionId, indexName, indexVersion },
          },
        },
      ),
    );
  }

  async getActiveCollectionPolicy(
    projectId: string,
    environmentId: string,
    collectionId: string,
  ): Promise<ActivePolicy> {
    return unwrap(
      await this.#client.GET(
        "/v1/projects/{projectId}/environments/{environmentId}/collections/{collectionId}/policies",
        { params: { path: { projectId, environmentId, collectionId } } },
      ),
    );
  }

  async createCollectionPolicyDraft(
    projectId: string,
    environmentId: string,
    collectionId: string,
    input: CreatePolicyDraftRequest,
    idempotencyKey: string,
  ): Promise<PolicySet> {
    return unwrap(
      await this.#client.POST(
        "/v1/projects/{projectId}/environments/{environmentId}/collections/{collectionId}/policies",
        {
          params: {
            path: { projectId, environmentId, collectionId },
            header: { "Idempotency-Key": idempotencyKey },
          },
          body: input,
        },
      ),
    );
  }

  async getCollectionPolicy(
    projectId: string,
    environmentId: string,
    collectionId: string,
    policyVersion: number,
  ): Promise<PolicySet> {
    return unwrap(
      await this.#client.GET(
        "/v1/projects/{projectId}/environments/{environmentId}/collections/{collectionId}/policies/{policyVersion}",
        { params: { path: { projectId, environmentId, collectionId, policyVersion } } },
      ),
    );
  }

  async validateCollectionPolicy(
    projectId: string,
    environmentId: string,
    collectionId: string,
    policyVersion: number,
  ): Promise<PolicyValidation> {
    return unwrap(
      await this.#client.POST(
        "/v1/projects/{projectId}/environments/{environmentId}/collections/{collectionId}/policies/{policyVersion}/actions/validate",
        { params: { path: { projectId, environmentId, collectionId, policyVersion } } },
      ),
    );
  }

  async testCollectionPolicy(
    projectId: string,
    environmentId: string,
    collectionId: string,
    policyVersion: number,
    examples: PolicyExample[],
  ): Promise<PolicyExampleResult[]> {
    const result = await this.#client.POST(
      "/v1/projects/{projectId}/environments/{environmentId}/collections/{collectionId}/policies/{policyVersion}/actions/test",
      {
        params: { path: { projectId, environmentId, collectionId, policyVersion } },
        body: { examples },
      },
    );
    return unwrap(result).results;
  }

  async activateCollectionPolicy(
    projectId: string,
    environmentId: string,
    collectionId: string,
    policyVersion: number,
    idempotencyKey: string,
  ): Promise<ActivePolicy> {
    return unwrap(
      await this.#client.POST(
        "/v1/projects/{projectId}/environments/{environmentId}/collections/{collectionId}/policies/{policyVersion}/actions/activate",
        {
          params: {
            path: { projectId, environmentId, collectionId, policyVersion },
            header: { "Idempotency-Key": idempotencyKey },
          },
        },
      ),
    );
  }

  async rollbackCollectionPolicy(
    projectId: string,
    environmentId: string,
    collectionId: string,
    policyVersion: number,
    idempotencyKey: string,
  ): Promise<ActivePolicy> {
    return unwrap(
      await this.#client.POST(
        "/v1/projects/{projectId}/environments/{environmentId}/collections/{collectionId}/policies/{policyVersion}/actions/rollback",
        {
          params: {
            path: { projectId, environmentId, collectionId, policyVersion },
            header: { "Idempotency-Key": idempotencyKey },
          },
        },
      ),
    );
  }

  async searchApplicationUsers(
    projectId: string,
    environmentId: string,
    options: { readonly query?: string; readonly limit?: number } = {},
  ): Promise<{ readonly users: ApplicationUserSummary[]; readonly truncated: boolean }> {
    return unwrap(
      await this.#client.GET("/v1/projects/{projectId}/environments/{environmentId}/users", {
        params: {
          path: { projectId, environmentId },
          query: options,
        },
      }),
    );
  }

  async createApplicationUser(
    projectId: string,
    environmentId: string,
    input: AdminCreateUserRequest,
    idempotencyKey: string,
  ): Promise<ApplicationUserView> {
    return unwrap(
      await this.#client.POST("/v1/projects/{projectId}/environments/{environmentId}/users", {
        params: {
          path: { projectId, environmentId },
          header: { "Idempotency-Key": idempotencyKey },
        },
        body: input,
      }),
    );
  }

  async inviteApplicationUser(
    projectId: string,
    environmentId: string,
    input: AdminCreateUserRequest,
    idempotencyKey: string,
  ): Promise<ApplicationUserView> {
    return unwrap(
      await this.#client.POST(
        "/v1/projects/{projectId}/environments/{environmentId}/users/invitations",
        {
          params: {
            path: { projectId, environmentId },
            header: { "Idempotency-Key": idempotencyKey },
          },
          body: input,
        },
      ),
    );
  }

  async getApplicationUser(
    projectId: string,
    environmentId: string,
    userId: string,
  ): Promise<ApplicationUserView> {
    return unwrap(
      await this.#client.GET(
        "/v1/projects/{projectId}/environments/{environmentId}/users/{userId}",
        { params: { path: { projectId, environmentId, userId } } },
      ),
    );
  }

  async updateApplicationUserMetadata(
    projectId: string,
    environmentId: string,
    userId: string,
    input: AdminUpdateUserMetadataRequest,
  ): Promise<ApplicationUserView> {
    return unwrap(
      await this.#client.PATCH(
        "/v1/projects/{projectId}/environments/{environmentId}/users/{userId}",
        {
          params: { path: { projectId, environmentId, userId } },
          body: input,
        },
      ),
    );
  }

  async deleteApplicationUser(
    projectId: string,
    environmentId: string,
    userId: string,
  ): Promise<ApplicationUserView> {
    return unwrap(
      await this.#client.DELETE(
        "/v1/projects/{projectId}/environments/{environmentId}/users/{userId}",
        { params: { path: { projectId, environmentId, userId } } },
      ),
    );
  }

  async disableApplicationUser(
    projectId: string,
    environmentId: string,
    userId: string,
  ): Promise<ApplicationUserView> {
    return unwrap(
      await this.#client.POST(
        "/v1/projects/{projectId}/environments/{environmentId}/users/{userId}/actions/disable",
        { params: { path: { projectId, environmentId, userId } } },
      ),
    );
  }

  async restoreApplicationUser(
    projectId: string,
    environmentId: string,
    userId: string,
  ): Promise<ApplicationUserView> {
    return unwrap(
      await this.#client.POST(
        "/v1/projects/{projectId}/environments/{environmentId}/users/{userId}/actions/restore",
        { params: { path: { projectId, environmentId, userId } } },
      ),
    );
  }

  async revokeApplicationUserSessions(
    projectId: string,
    environmentId: string,
    userId: string,
  ): Promise<ApplicationUserView> {
    return unwrap(
      await this.#client.POST(
        "/v1/projects/{projectId}/environments/{environmentId}/users/{userId}/actions/revoke-sessions",
        { params: { path: { projectId, environmentId, userId } } },
      ),
    );
  }

  async revokeApplicationUserSession(
    projectId: string,
    environmentId: string,
    userId: string,
    sessionId: string,
  ): Promise<ApplicationUserView> {
    return unwrap(
      await this.#client.DELETE(
        "/v1/projects/{projectId}/environments/{environmentId}/users/{userId}/sessions/{sessionId}",
        { params: { path: { projectId, environmentId, userId, sessionId } } },
      ),
    );
  }

  async createPublicProjectKey(
    projectId: string,
    environmentId: string,
    id: string,
    idempotencyKey: string,
  ): Promise<ProjectCredentialIssue> {
    return unwrap(
      await this.#client.POST(
        "/v1/projects/{projectId}/environments/{environmentId}/credentials/public",
        {
          params: {
            path: { projectId, environmentId },
            header: { "Idempotency-Key": idempotencyKey },
          },
          body: { id },
        },
      ),
    );
  }

  async createServiceCredential(
    projectId: string,
    environmentId: string,
    id: string,
    scope: ServiceCredentialScope,
    idempotencyKey: string,
  ): Promise<ProjectCredentialIssue> {
    return unwrap(
      await this.#client.POST(
        "/v1/projects/{projectId}/environments/{environmentId}/credentials/service",
        {
          params: {
            path: { projectId, environmentId },
            header: { "Idempotency-Key": idempotencyKey },
          },
          body: { id, scope },
        },
      ),
    );
  }

  async getProjectCredential(
    projectId: string,
    environmentId: string,
    credentialId: string,
  ): Promise<ProjectCredential> {
    return unwrap(
      await this.#client.GET(
        "/v1/projects/{projectId}/environments/{environmentId}/credentials/{credentialId}",
        { params: { path: { projectId, environmentId, credentialId } } },
      ),
    );
  }

  async retireProjectCredential(
    projectId: string,
    environmentId: string,
    credentialId: string,
  ): Promise<void> {
    expectNoContent(
      await this.#client.DELETE(
        "/v1/projects/{projectId}/environments/{environmentId}/credentials/{credentialId}",
        { params: { path: { projectId, environmentId, credentialId } } },
      ),
    );
  }

  async rotateProjectCredential(
    projectId: string,
    environmentId: string,
    credentialId: string,
    replacementId: string,
    overlapSeconds: number,
    idempotencyKey: string,
  ): Promise<ProjectCredentialIssue> {
    return unwrap(
      await this.#client.POST(
        "/v1/projects/{projectId}/environments/{environmentId}/credentials/{credentialId}/actions/rotate",
        {
          params: {
            path: { projectId, environmentId, credentialId },
            header: { "Idempotency-Key": idempotencyKey },
          },
          body: { replacementId, overlapSeconds },
        },
      ),
    );
  }

  async initializeJwtSigningKey(
    projectId: string,
    environmentId: string,
    idempotencyKey: string,
  ): Promise<JwtSigningKey> {
    return unwrap(
      await this.#client.POST(
        "/v1/projects/{projectId}/environments/{environmentId}/signing-keys/actions/initialize",
        {
          params: {
            path: { projectId, environmentId },
            header: { "Idempotency-Key": idempotencyKey },
          },
        },
      ),
    );
  }

  async listJwtSigningKeys(projectId: string, environmentId: string): Promise<JwtSigningKey[]> {
    const result = await this.#client.GET(
      "/v1/projects/{projectId}/environments/{environmentId}/signing-keys",
      { params: { path: { projectId, environmentId } } },
    );
    return unwrap(result).items;
  }

  async rotateJwtSigningKey(
    projectId: string,
    environmentId: string,
    overlapSeconds: number,
    idempotencyKey: string,
  ): Promise<JwtSigningKey> {
    return unwrap(
      await this.#client.POST(
        "/v1/projects/{projectId}/environments/{environmentId}/signing-keys",
        {
          params: {
            path: { projectId, environmentId },
            header: { "Idempotency-Key": idempotencyKey },
          },
          body: { overlapSeconds },
        },
      ),
    );
  }

  async createFunctionSecret(
    projectId: string,
    environmentId: string,
    name: string,
    idempotencyKey: string,
  ): Promise<FunctionSecretIssue> {
    return unwrap(
      await this.#client.POST(
        "/v1/projects/{projectId}/environments/{environmentId}/function-secrets",
        {
          params: {
            path: { projectId, environmentId },
            header: { "Idempotency-Key": idempotencyKey },
          },
          body: { name },
        },
      ),
    );
  }

  async getFunctionSecret(
    projectId: string,
    environmentId: string,
    secretName: string,
  ): Promise<FunctionSecret> {
    return unwrap(
      await this.#client.GET(
        "/v1/projects/{projectId}/environments/{environmentId}/function-secrets/{secretName}",
        { params: { path: { projectId, environmentId, secretName } } },
      ),
    );
  }

  async retireFunctionSecret(
    projectId: string,
    environmentId: string,
    secretName: string,
  ): Promise<FunctionSecret> {
    return unwrap(
      await this.#client.DELETE(
        "/v1/projects/{projectId}/environments/{environmentId}/function-secrets/{secretName}",
        { params: { path: { projectId, environmentId, secretName } } },
      ),
    );
  }

  async rotateFunctionSecret(
    projectId: string,
    environmentId: string,
    secretName: string,
    idempotencyKey: string,
  ): Promise<FunctionSecretIssue> {
    return unwrap(
      await this.#client.POST(
        "/v1/projects/{projectId}/environments/{environmentId}/function-secrets/{secretName}/actions/rotate",
        {
          params: {
            path: { projectId, environmentId, secretName },
            header: { "Idempotency-Key": idempotencyKey },
          },
        },
      ),
    );
  }

  async uploadFunctionBundle(
    projectId: string,
    environmentId: string,
    input: FunctionBundleUploadRequest,
    idempotencyKey: string,
  ): Promise<FunctionBundleUploadResult> {
    return unwrap(
      await this.#client.POST(
        "/v1/projects/{projectId}/environments/{environmentId}/function-bundles",
        {
          params: {
            path: { projectId, environmentId },
            header: { "Idempotency-Key": idempotencyKey },
          },
          body: input,
        },
      ),
    );
  }

  async listFunctions(projectId: string, environmentId: string): Promise<Function[]> {
    const result = await this.#client.GET(
      "/v1/projects/{projectId}/environments/{environmentId}/functions",
      { params: { path: { projectId, environmentId } } },
    );
    return unwrap(result).items;
  }

  async createFunction(
    projectId: string,
    environmentId: string,
    input: CreateFunctionRequest,
    idempotencyKey: string,
  ): Promise<Function> {
    return unwrap(
      await this.#client.POST("/v1/projects/{projectId}/environments/{environmentId}/functions", {
        params: {
          path: { projectId, environmentId },
          header: { "Idempotency-Key": idempotencyKey },
        },
        body: input,
      }),
    );
  }

  async getFunction(
    projectId: string,
    environmentId: string,
    functionName: string,
  ): Promise<Function> {
    return unwrap(
      await this.#client.GET(
        "/v1/projects/{projectId}/environments/{environmentId}/functions/{functionName}",
        { params: { path: { projectId, environmentId, functionName } } },
      ),
    );
  }

  async updateFunctionConfiguration(
    projectId: string,
    environmentId: string,
    functionName: string,
    configuration: FunctionConfiguration,
  ): Promise<Function> {
    return unwrap(
      await this.#client.PATCH(
        "/v1/projects/{projectId}/environments/{environmentId}/functions/{functionName}",
        {
          params: { path: { projectId, environmentId, functionName } },
          body: configuration,
        },
      ),
    );
  }

  async deleteFunction(
    projectId: string,
    environmentId: string,
    functionName: string,
  ): Promise<Function> {
    return unwrap(
      await this.#client.DELETE(
        "/v1/projects/{projectId}/environments/{environmentId}/functions/{functionName}",
        { params: { path: { projectId, environmentId, functionName } } },
      ),
    );
  }

  async listFunctionDeployments(
    projectId: string,
    environmentId: string,
    functionName: string,
  ): Promise<FunctionDeployment[]> {
    const result = await this.#client.GET(
      "/v1/projects/{projectId}/environments/{environmentId}/functions/{functionName}/versions",
      { params: { path: { projectId, environmentId, functionName } } },
    );
    return unwrap(result).items;
  }

  async createFunctionDeployment(
    projectId: string,
    environmentId: string,
    functionName: string,
    input: FunctionDeploymentRequest,
    idempotencyKey: string,
  ): Promise<FunctionDeployment> {
    return unwrap(
      await this.#client.POST(
        "/v1/projects/{projectId}/environments/{environmentId}/functions/{functionName}/versions",
        {
          params: {
            path: { projectId, environmentId, functionName },
            header: { "Idempotency-Key": idempotencyKey },
          },
          body: input,
        },
      ),
    );
  }

  async getFunctionDeployment(
    projectId: string,
    environmentId: string,
    functionName: string,
    functionVersion: number,
  ): Promise<FunctionDeployment> {
    return unwrap(
      await this.#client.GET(
        "/v1/projects/{projectId}/environments/{environmentId}/functions/{functionName}/versions/{functionVersion}",
        { params: { path: { projectId, environmentId, functionName, functionVersion } } },
      ),
    );
  }

  async deleteFunctionDeployment(
    projectId: string,
    environmentId: string,
    functionName: string,
    functionVersion: number,
  ): Promise<void> {
    expectNoContent(
      await this.#client.DELETE(
        "/v1/projects/{projectId}/environments/{environmentId}/functions/{functionName}/versions/{functionVersion}",
        { params: { path: { projectId, environmentId, functionName, functionVersion } } },
      ),
    );
  }

  async checkFunctionDeploymentHealth(
    projectId: string,
    environmentId: string,
    functionName: string,
    functionVersion: number,
  ): Promise<FunctionDeployment> {
    return unwrap(
      await this.#client.POST(
        "/v1/projects/{projectId}/environments/{environmentId}/functions/{functionName}/versions/{functionVersion}/actions/health-check",
        { params: { path: { projectId, environmentId, functionName, functionVersion } } },
      ),
    );
  }

  async promoteFunctionDeployment(
    projectId: string,
    environmentId: string,
    functionName: string,
    functionVersion: number,
    idempotencyKey: string,
  ): Promise<Function> {
    return this.#switchFunctionDeployment(
      "promote",
      projectId,
      environmentId,
      functionName,
      functionVersion,
      idempotencyKey,
    );
  }

  async rollbackFunctionDeployment(
    projectId: string,
    environmentId: string,
    functionName: string,
    functionVersion: number,
    idempotencyKey: string,
  ): Promise<Function> {
    return this.#switchFunctionDeployment(
      "rollback",
      projectId,
      environmentId,
      functionName,
      functionVersion,
      idempotencyKey,
    );
  }

  async testFunctionInvocation(
    projectId: string,
    environmentId: string,
    functionName: string,
    input: FunctionTestRequest,
  ): Promise<FunctionTestResponse> {
    return unwrap(
      await this.#client.POST(
        "/v1/projects/{projectId}/environments/{environmentId}/functions/{functionName}/actions/test",
        {
          params: { path: { projectId, environmentId, functionName } },
          body: input,
        },
      ),
    );
  }

  async queryFunctionLogs(
    projectId: string,
    environmentId: string,
    functionName: string,
    query: { readonly cursor?: string; readonly limit?: number } = {},
  ): Promise<FunctionLogPage> {
    return unwrap(
      await this.#client.GET(
        "/v1/projects/{projectId}/environments/{environmentId}/functions/{functionName}/logs",
        { params: { path: { projectId, environmentId, functionName }, query } },
      ),
    );
  }

  async queryProjectUsage(
    projectId: string,
    environmentId: string,
    query: ObservabilityQuery = {},
  ): Promise<ObservabilityPage> {
    return unwrap(
      await this.#client.GET(
        "/v1/projects/{projectId}/environments/{environmentId}/observability/usage",
        { params: { path: { projectId, environmentId }, query } },
      ),
    );
  }

  async queryProjectQuotas(
    projectId: string,
    environmentId: string,
    query: ObservabilityQuery = {},
  ): Promise<ObservabilityPage> {
    return unwrap(
      await this.#client.GET(
        "/v1/projects/{projectId}/environments/{environmentId}/observability/quotas",
        { params: { path: { projectId, environmentId }, query } },
      ),
    );
  }

  async queryProjectHealth(
    projectId: string,
    environmentId: string,
    query: ObservabilityQuery = {},
  ): Promise<ObservabilityPage> {
    return unwrap(
      await this.#client.GET(
        "/v1/projects/{projectId}/environments/{environmentId}/observability/health",
        { params: { path: { projectId, environmentId }, query } },
      ),
    );
  }

  async queryReplicationErrors(
    projectId: string,
    environmentId: string,
    query: ObservabilityQuery = {},
  ): Promise<ObservabilityPage> {
    return unwrap(
      await this.#client.GET(
        "/v1/projects/{projectId}/environments/{environmentId}/observability/replication-errors",
        { params: { path: { projectId, environmentId }, query } },
      ),
    );
  }

  async queryAuthenticationEvents(
    projectId: string,
    environmentId: string,
    query: ObservabilityQuery = {},
  ): Promise<ObservabilityPage> {
    return unwrap(
      await this.#client.GET(
        "/v1/projects/{projectId}/environments/{environmentId}/observability/auth-events",
        { params: { path: { projectId, environmentId }, query } },
      ),
    );
  }

  async queryFunctionMetrics(
    projectId: string,
    environmentId: string,
    query: ObservabilityQuery = {},
  ): Promise<ObservabilityPage> {
    return unwrap(
      await this.#client.GET(
        "/v1/projects/{projectId}/environments/{environmentId}/observability/function-metrics",
        { params: { path: { projectId, environmentId }, query } },
      ),
    );
  }

  async queryProjectLogs(
    projectId: string,
    environmentId: string,
    query: ObservabilityQuery = {},
  ): Promise<ObservabilityPage> {
    return unwrap(
      await this.#client.GET(
        "/v1/projects/{projectId}/environments/{environmentId}/observability/logs",
        { params: { path: { projectId, environmentId }, query } },
      ),
    );
  }

  async queryIndexStateEvents(
    projectId: string,
    environmentId: string,
    query: ObservabilityQuery = {},
  ): Promise<ObservabilityPage> {
    return unwrap(
      await this.#client.GET(
        "/v1/projects/{projectId}/environments/{environmentId}/observability/index-states",
        { params: { path: { projectId, environmentId }, query } },
      ),
    );
  }

  async queryAuditEvents(
    projectId: string,
    environmentId: string,
    query: ObservabilityQuery = {},
  ): Promise<ObservabilityPage> {
    return unwrap(
      await this.#client.GET(
        "/v1/projects/{projectId}/environments/{environmentId}/observability/audit-events",
        { params: { path: { projectId, environmentId }, query } },
      ),
    );
  }

  async issueExplorerGrant(
    projectId: string,
    environmentId: string,
    input: ExplorerGrantRequest,
  ): Promise<ExplorerGrant> {
    return unwrap(
      await this.#client.POST(
        "/v1/projects/{projectId}/environments/{environmentId}/explorer/grants",
        { params: { path: { projectId, environmentId } }, body: input },
      ),
    );
  }

  async revokeExplorerGrant(
    projectId: string,
    environmentId: string,
    grantId: string,
  ): Promise<ExplorerGrantRevocation> {
    return unwrap(
      await this.#client.DELETE(
        "/v1/projects/{projectId}/environments/{environmentId}/explorer/grants/{grantId}",
        { params: { path: { projectId, environmentId, grantId } } },
      ),
    );
  }

  async explorerGetDocument(
    projectId: string,
    environmentId: string,
    collectionId: string,
    documentId: string,
    capability: string,
  ): Promise<ExplorerDocument> {
    return unwrap(
      await this.#client.GET(
        "/v1/projects/{projectId}/environments/{environmentId}/explorer/collections/{collectionId}/documents/{documentId}",
        {
          params: { path: { projectId, environmentId, collectionId, documentId } },
          headers: explorerHeaders(capability),
        },
      ),
    );
  }

  async explorerBrowseDocuments(
    projectId: string,
    environmentId: string,
    collectionId: string,
    input: ExplorerPageRequest,
    capability: string,
  ): Promise<ExplorerDocumentPage> {
    return unwrap(
      await this.#client.POST(
        "/v1/projects/{projectId}/environments/{environmentId}/explorer/collections/{collectionId}/browse",
        {
          params: { path: { projectId, environmentId, collectionId } },
          headers: explorerHeaders(capability),
          body: input,
        },
      ),
    );
  }

  async explorerPlanQuery(
    projectId: string,
    environmentId: string,
    collectionId: string,
    input: ExplorerQueryRequest,
    capability: string,
  ): Promise<ExplorerQueryPlan> {
    return unwrap(
      await this.#client.POST(
        "/v1/projects/{projectId}/environments/{environmentId}/explorer/collections/{collectionId}/query/plan",
        {
          params: { path: { projectId, environmentId, collectionId } },
          headers: explorerHeaders(capability),
          body: input,
        },
      ),
    );
  }

  async explorerQueryDocuments(
    projectId: string,
    environmentId: string,
    collectionId: string,
    input: ExplorerQueryRequest,
    capability: string,
  ): Promise<ExplorerDocumentPage> {
    return unwrap(
      await this.#client.POST(
        "/v1/projects/{projectId}/environments/{environmentId}/explorer/collections/{collectionId}/query",
        {
          params: { path: { projectId, environmentId, collectionId } },
          headers: explorerHeaders(capability),
          body: input,
        },
      ),
    );
  }

  async explorerDocumentHistory(
    projectId: string,
    environmentId: string,
    collectionId: string,
    documentId: string,
    capability: string,
  ): Promise<ExplorerRevision[]> {
    return unwrap(
      await this.#client.GET(
        "/v1/projects/{projectId}/environments/{environmentId}/explorer/collections/{collectionId}/documents/{documentId}/history",
        {
          params: { path: { projectId, environmentId, collectionId, documentId } },
          headers: explorerHeaders(capability),
        },
      ),
    );
  }

  async explorerSimulateMutation(
    projectId: string,
    environmentId: string,
    collectionId: string,
    input: ExplorerMutationRequest,
    capability: string,
  ): Promise<ExplorerSimulation> {
    return unwrap(
      await this.#client.POST(
        "/v1/projects/{projectId}/environments/{environmentId}/explorer/collections/{collectionId}/simulate",
        {
          params: { path: { projectId, environmentId, collectionId } },
          headers: explorerHeaders(capability),
          body: input,
        },
      ),
    );
  }

  async explorerMutateDocument(
    projectId: string,
    environmentId: string,
    collectionId: string,
    input: ExplorerMutationRequest,
    capability: string,
  ): Promise<ExplorerMutationResult> {
    return unwrap(
      await this.#client.POST(
        "/v1/projects/{projectId}/environments/{environmentId}/explorer/collections/{collectionId}/mutate",
        {
          params: {
            path: { projectId, environmentId, collectionId },
            header: { "Idempotency-Key": input.idempotencyKey },
          },
          headers: explorerHeaders(capability),
          body: input,
        },
      ),
    );
  }

  async listDataJobs(projectId: string, environmentId: string): Promise<DataJob[]> {
    const page = unwrap(
      await this.#client.GET("/v1/projects/{projectId}/environments/{environmentId}/data-jobs", {
        params: { path: { projectId, environmentId } },
      }),
    );
    return page.items;
  }

  async createDataJob(
    projectId: string,
    environmentId: string,
    input: DataJobCreateRequest,
    idempotencyKey: string,
  ): Promise<DataJob> {
    return unwrap(
      await this.#client.POST("/v1/projects/{projectId}/environments/{environmentId}/data-jobs", {
        params: {
          path: { projectId, environmentId },
          header: { "Idempotency-Key": idempotencyKey },
        },
        body: input,
      }),
    );
  }

  async getDataJob(projectId: string, environmentId: string, jobId: string): Promise<DataJob> {
    return unwrap(
      await this.#client.GET(
        "/v1/projects/{projectId}/environments/{environmentId}/data-jobs/{jobId}",
        { params: { path: { projectId, environmentId, jobId } } },
      ),
    );
  }

  async cancelDataJob(projectId: string, environmentId: string, jobId: string): Promise<DataJob> {
    return unwrap(
      await this.#client.POST(
        "/v1/projects/{projectId}/environments/{environmentId}/data-jobs/{jobId}/actions/cancel",
        { params: { path: { projectId, environmentId, jobId } } },
      ),
    );
  }

  async dryRunDataJobImport(
    projectId: string,
    environmentId: string,
    jobId: string,
    input: DataJobDryRunRequest,
  ): Promise<DataJob> {
    return unwrap(
      await this.#client.POST(
        "/v1/projects/{projectId}/environments/{environmentId}/data-jobs/{jobId}/actions/dry-run",
        { params: { path: { projectId, environmentId, jobId } }, body: input },
      ),
    );
  }

  async confirmDataJob(
    projectId: string,
    environmentId: string,
    jobId: string,
    input: DataJobConfirmationRequest,
  ): Promise<DataJob> {
    return unwrap(
      await this.#client.POST(
        "/v1/projects/{projectId}/environments/{environmentId}/data-jobs/{jobId}/actions/confirm",
        { params: { path: { projectId, environmentId, jobId } }, body: input },
      ),
    );
  }

  async createDataJobUploadGrant(
    projectId: string,
    environmentId: string,
    jobId: string,
  ): Promise<ArtifactGrant> {
    return unwrap(
      await this.#client.POST(
        "/v1/projects/{projectId}/environments/{environmentId}/data-jobs/{jobId}/artifact-grants/upload",
        { params: { path: { projectId, environmentId, jobId } } },
      ),
    );
  }

  async createDataJobDownloadGrant(
    projectId: string,
    environmentId: string,
    jobId: string,
  ): Promise<ArtifactGrant> {
    return unwrap(
      await this.#client.POST(
        "/v1/projects/{projectId}/environments/{environmentId}/data-jobs/{jobId}/artifact-grants/download",
        { params: { path: { projectId, environmentId, jobId } } },
      ),
    );
  }

  async getWorkspaceSummary(projectId: string, environmentId: string): Promise<WorkspaceSummary> {
    return unwrap(
      await this.#client.GET(
        "/v1/projects/{projectId}/environments/{environmentId}/workspace/summary",
        { params: { path: { projectId, environmentId } } },
      ),
    );
  }

  async getWorkspaceNavigation(
    projectId: string,
    environmentId: string,
  ): Promise<WorkspaceDestination[]> {
    return unwrap(
      await this.#client.GET(
        "/v1/projects/{projectId}/environments/{environmentId}/workspace/navigation",
        { params: { path: { projectId, environmentId } } },
      ),
    );
  }

  async getConnectMetadata(projectId: string, environmentId: string): Promise<ConnectMetadata> {
    return unwrap(
      await this.#client.GET("/v1/projects/{projectId}/environments/{environmentId}/connect", {
        params: { path: { projectId, environmentId } },
      }),
    );
  }

  async checkConnection(
    projectId: string,
    environmentId: string,
    input: ConnectionCheckRequest = {},
  ): Promise<ConnectionCheck> {
    return unwrap(
      await this.#client.POST(
        "/v1/projects/{projectId}/environments/{environmentId}/connect/check",
        { params: { path: { projectId, environmentId } }, body: input },
      ),
    );
  }

  async getSyncSummary(
    projectId: string,
    environmentId: string,
    query: { readonly collectionId?: string; readonly from?: number; readonly until?: number } = {},
  ): Promise<SyncSummary> {
    return unwrap(
      await this.#client.GET("/v1/projects/{projectId}/environments/{environmentId}/sync/summary", {
        params: { path: { projectId, environmentId }, query },
      }),
    );
  }

  async listDeveloperBackups(projectId: string, environmentId: string): Promise<DeveloperBackup[]> {
    return unwrap(
      await this.#client.GET("/v1/projects/{projectId}/environments/{environmentId}/backups", {
        params: { path: { projectId, environmentId } },
      }),
    );
  }

  async listDeveloperRestoreRequests(projectId: string): Promise<DeveloperRestore[]> {
    return unwrap(
      await this.#client.GET("/v1/projects/{projectId}/restore-requests", {
        params: { path: { projectId } },
      }),
    );
  }

  async requestDeveloperRestore(
    projectId: string,
    input: DeveloperRestoreRequest,
    idempotencyKey: string,
  ): Promise<DeveloperRestore> {
    return unwrap(
      await this.#client.POST("/v1/projects/{projectId}/restore-requests", {
        params: {
          path: { projectId },
          header: { "Idempotency-Key": idempotencyKey },
        },
        body: input,
      }),
    );
  }

  async verifyCurrentDeveloperPassword(password: string): Promise<DeveloperStepUpGrant> {
    return unwrap(
      await this.#client.POST("/v1/developer-auth/sessions/current/actions/verify-password", {
        body: { password },
      }),
    );
  }

  async #switchFunctionDeployment(
    action: "promote" | "rollback",
    projectId: string,
    environmentId: string,
    functionName: string,
    functionVersion: number,
    idempotencyKey: string,
  ): Promise<Function> {
    const params = {
      path: { projectId, environmentId, functionName, functionVersion },
      header: { "Idempotency-Key": idempotencyKey },
    };
    return action === "promote"
      ? unwrap(
          await this.#client.POST(
            "/v1/projects/{projectId}/environments/{environmentId}/functions/{functionName}/versions/{functionVersion}/actions/promote",
            { params },
          ),
        )
      : unwrap(
          await this.#client.POST(
            "/v1/projects/{projectId}/environments/{environmentId}/functions/{functionName}/versions/{functionVersion}/actions/rollback",
            { params },
          ),
        );
  }

  async #accessToken(): Promise<string> {
    const provider = this.#credential.accessToken;
    const token = typeof provider === "string" ? provider : await provider();
    if (
      token.length < 16 ||
      token.length > 16 * 1024 ||
      Array.from(token).some((character) => /\s/u.test(character) || isControl(character))
    ) {
      throw new TypeError(`${this.#credential.kind} credential is invalid`);
    }
    return token;
  }
}

export function createManagementClient(options: ManagementClientOptions): MakoManagementClient {
  return new MakoManagementClient(options);
}

export class MakoDeveloperAuthClient {
  readonly #client: MakoApiClient;

  constructor(options: DeveloperAuthClientOptions) {
    const baseUrl = normalizeEndpoint(options.endpoint);
    const requestFetch = options.fetch;
    this.#client =
      requestFetch === undefined
        ? createMakoApiClient({ baseUrl })
        : createMakoApiClient({ baseUrl, fetch: (request) => requestFetch(request) });
    this.#client.use({
      onRequest: ({ request }) => {
        request.headers.set("Accept", "application/json");
        return request;
      },
    });
  }

  async register(input: DeveloperRegistrationRequest): Promise<DeveloperGenericAccepted> {
    return unwrap(await this.#client.POST("/v1/developer-auth/registrations", { body: input }));
  }

  async verifyEmail(
    token: string,
  ): Promise<{ readonly status: "waitlisted" | "password_updated" }> {
    return unwrap(await this.#client.POST("/v1/developer-auth/verifications", { body: { token } }));
  }

  async resendVerification(email: string): Promise<DeveloperGenericAccepted> {
    return unwrap(
      await this.#client.POST("/v1/developer-auth/verification-resends", { body: { email } }),
    );
  }

  async signIn(email: string, password: string): Promise<DeveloperSession> {
    return unwrap(
      await this.#client.POST("/v1/developer-auth/sessions", { body: { email, password } }),
    );
  }

  async refresh(): Promise<DeveloperSession> {
    return unwrap(await this.#client.POST("/v1/developer-auth/sessions/refresh"));
  }

  async signOut(): Promise<void> {
    expectNoContent(await this.#client.DELETE("/v1/developer-auth/sessions/current"));
  }

  async requestPasswordRecovery(email: string): Promise<DeveloperGenericAccepted> {
    return unwrap(
      await this.#client.POST("/v1/developer-auth/password-recovery-requests", {
        body: { email },
      }),
    );
  }

  async completePasswordRecovery(
    token: string,
    password: string,
  ): Promise<{ readonly status: "waitlisted" | "password_updated" }> {
    return unwrap(
      await this.#client.POST("/v1/developer-auth/password-recoveries", {
        body: { token, password },
      }),
    );
  }

  async waitListStatus(accessToken: string): Promise<DeveloperWaitListStatus> {
    return unwrap(
      await this.#client.GET("/v1/developer-auth/wait-list-status", {
        headers: { Authorization: `Bearer ${validateAccessToken(accessToken)}` },
      }),
    );
  }
}

export function createDeveloperAuthClient(
  options: DeveloperAuthClientOptions,
): MakoDeveloperAuthClient {
  return new MakoDeveloperAuthClient(options);
}

export class MakoOperatorClient {
  readonly #client: MakoApiClient;

  constructor(options: OperatorClientOptions) {
    const baseUrl = normalizeEndpoint(options.endpoint);
    const requestFetch = options.fetch;
    this.#client =
      requestFetch === undefined
        ? createMakoApiClient({ baseUrl })
        : createMakoApiClient({ baseUrl, fetch: (request) => requestFetch(request) });
    this.#client.use({
      onRequest: ({ request }) => {
        request.headers.set("Accept", "application/json");
        return new Request(request, { credentials: "include" });
      },
    });
  }

  async signIn(email: string, password: string): Promise<OperatorSession> {
    return unwrap(
      await this.#client.POST("/v1/operator-auth/sessions", { body: { email, password } }),
    );
  }

  async currentSession(): Promise<OperatorSession> {
    return unwrap(await this.#client.GET("/v1/operator-auth/sessions/current"));
  }

  async signOut(): Promise<void> {
    expectNoContent(await this.#client.DELETE("/v1/operator-auth/sessions/current"));
  }

  async verifyPassword(password: string): Promise<OperatorSession> {
    return unwrap(
      await this.#client.POST("/v1/operator-auth/sessions/current/actions/verify-password", {
        body: { password },
      }),
    );
  }

  async getOperatorOverview(): Promise<OperatorOverview> {
    return unwrap(await this.#client.GET("/v1/operator/overview"));
  }

  async listOperatorTenants(
    input: {
      readonly query?: string;
      readonly lifecycle?: components["schemas"]["LifecycleState"];
      readonly health?: components["schemas"]["OperatorFreshness"];
      readonly region?: string;
      readonly plan?: string;
      readonly quotaState?: string;
      readonly cursor?: string;
      readonly limit?: number;
    } = {},
  ): Promise<OperatorTenantPage> {
    return unwrap(await this.#client.GET("/v1/operator/tenants", { params: { query: input } }));
  }

  async getOperatorTenant360(projectId: string): Promise<OperatorTenant360> {
    return unwrap(
      await this.#client.GET("/v1/operator/tenants/{projectId}", {
        params: { path: { projectId } },
      }),
    );
  }

  async getOperatorInventory(
    inventoryKind: OperatorInventoryKind,
    input: { readonly projectId?: string; readonly from?: number; readonly until?: number } = {},
  ): Promise<{ readonly items: OperatorReadSection[]; readonly observedAt: string }> {
    return unwrap(
      await this.#client.GET("/v1/operator/inventory/{inventoryKind}", {
        params: { path: { inventoryKind }, query: input },
      }),
    );
  }

  async listOperatorIncidents(
    input: {
      readonly cursor?: string;
      readonly limit?: number;
      readonly state?: "open" | "acknowledged" | "resolved";
      readonly severity?: "critical" | "high" | "medium" | "low";
      readonly projectId?: string;
      readonly from?: number;
      readonly until?: number;
    } = {},
  ): Promise<OperatorIncidentPage> {
    return unwrap(await this.#client.GET("/v1/operator/incidents", { params: { query: input } }));
  }

  async getOperatorIncident(incidentId: string): Promise<OperatorIncident> {
    return unwrap(
      await this.#client.GET("/v1/operator/incidents/{incidentId}", {
        params: { path: { incidentId } },
      }),
    );
  }

  async listOperatorAlerts(
    input: { readonly projectId?: string; readonly cursor?: string; readonly limit?: number } = {},
  ): Promise<OperatorAlertPage> {
    return unwrap(await this.#client.GET("/v1/operator/alerts", { params: { query: input } }));
  }

  async getOperatorAlert(alertFingerprint: string, projectId?: string): Promise<OperatorAlert> {
    return unwrap(
      await this.#client.GET("/v1/operator/alerts/{alertFingerprint}", {
        params: {
          path: { alertFingerprint },
          query: projectId === undefined ? {} : { projectId },
        },
      }),
    );
  }

  async createOperatorIncident(input: CreateOperatorIncidentRequest): Promise<OperatorIncident> {
    return unwrap(await this.#client.POST("/v1/operator/incidents", { body: input }));
  }

  async updateOperatorIncident(
    incidentId: string,
    input: UpdateOperatorIncidentRequest,
  ): Promise<OperatorIncident> {
    return unwrap(
      await this.#client.POST("/v1/operator/incidents/{incidentId}/actions/update", {
        params: { path: { incidentId } },
        body: input,
      }),
    );
  }

  async createOperatorRecoveryJob(
    input: CreateOperatorRecoveryJobRequest,
  ): Promise<OperatorRecoveryJob> {
    return unwrap(await this.#client.POST("/v1/operator/recovery-jobs", { body: input }));
  }

  async listOperatorRecoveryJobs(
    input: { readonly projectId?: string; readonly cursor?: string; readonly limit?: number } = {},
  ): Promise<OperatorRecoveryJobPage> {
    return unwrap(
      await this.#client.GET("/v1/operator/recovery-jobs", { params: { query: input } }),
    );
  }

  async getOperatorRecoveryJob(recoveryJobId: string): Promise<OperatorRecoveryJob> {
    return unwrap(
      await this.#client.GET("/v1/operator/recovery-jobs/{recoveryJobId}", {
        params: { path: { recoveryJobId } },
      }),
    );
  }

  async advanceOperatorRecoveryJob(
    recoveryJobId: string,
    input: AdvanceOperatorRecoveryJobRequest,
  ): Promise<OperatorRecoveryJob> {
    return unwrap(
      await this.#client.POST("/v1/operator/recovery-jobs/{recoveryJobId}/actions/advance", {
        params: { path: { recoveryJobId } },
        body: input,
      }),
    );
  }

  async listOperatorActivity(
    input: {
      readonly query?: string;
      readonly from?: number;
      readonly until?: number;
      readonly cursor?: string;
      readonly limit?: number;
    } = {},
  ): Promise<OperatorActivityPage> {
    return unwrap(await this.#client.GET("/v1/operator/activity", { params: { query: input } }));
  }

  async createOperatorActivityExport(
    input: CreateOperatorActivityExportRequest,
  ): Promise<OperatorActivityExport> {
    return unwrap(await this.#client.POST("/v1/operator/activity-exports", { body: input }));
  }

  async getOperatorActivityExport(activityExportId: string): Promise<OperatorActivityExport> {
    return unwrap(
      await this.#client.GET("/v1/operator/activity-exports/{activityExportId}", {
        params: { path: { activityExportId } },
      }),
    );
  }

  async processOperatorActivityExport(activityExportId: string): Promise<OperatorActivityExport> {
    return unwrap(
      await this.#client.POST("/v1/operator/activity-exports/{activityExportId}/actions/process", {
        params: { path: { activityExportId } },
      }),
    );
  }

  async rebuildOperatorProjection(
    projectionName: "tenant_search" | "activity",
    cursor?: string | null,
  ): Promise<OperatorProjectionEvidence> {
    return unwrap(
      await this.#client.POST("/v1/operator/projections/{projectionName}/actions/rebuild", {
        params: { path: { projectionName } },
        body: cursor === undefined ? {} : { cursor },
      }),
    );
  }

  async listOperatorProvisioningWorkflows(
    input: { readonly projectId?: string; readonly limit?: number } = {},
  ): Promise<ProvisioningWorkflow[]> {
    return unwrap(
      await this.#client.GET("/v1/operator/provisioning-workflows", {
        params: { query: input },
      }),
    ).items;
  }

  async listOperatorQuotaOverrides(projectId: string, limit = 25): Promise<QuotaOverride[]> {
    return unwrap(
      await this.#client.GET("/v1/operator/projects/{projectId}/quota-overrides", {
        params: { path: { projectId }, query: { limit } },
      }),
    ).items;
  }

  async replaceOperatorQuotaOverride(
    projectId: string,
    quotaOverrideId: string,
    input: {
      readonly reviewedVersion: number;
      readonly limit: number;
      readonly expiresAt: string | null;
      readonly reason: string;
    },
  ): Promise<QuotaOverride> {
    return unwrap(
      await this.#client.POST(
        "/v1/operator/projects/{projectId}/quota-overrides/{quotaOverrideId}/actions/replace",
        { params: { path: { projectId, quotaOverrideId } }, body: input },
      ),
    );
  }

  async revokeOperatorQuotaOverride(
    projectId: string,
    quotaOverrideId: string,
    reviewedVersion: number,
    reason: string,
  ): Promise<QuotaOverride> {
    return unwrap(
      await this.#client.POST(
        "/v1/operator/projects/{projectId}/quota-overrides/{quotaOverrideId}/actions/revoke",
        {
          params: { path: { projectId, quotaOverrideId } },
          body: { reviewedVersion, reason },
        },
      ),
    );
  }

  async listOperatorAbuseResponses(projectId: string, limit = 25): Promise<AbuseResponse[]> {
    return unwrap(
      await this.#client.GET("/v1/operator/projects/{projectId}/abuse-responses", {
        params: { path: { projectId }, query: { limit } },
      }),
    ).items;
  }

  async restoreOperatorAbuseResponse(
    projectId: string,
    abuseResponseId: string,
    reviewedVersion: number,
    reason: string,
  ): Promise<AbuseResponse> {
    return unwrap(
      await this.#client.POST(
        "/v1/operator/projects/{projectId}/abuse-responses/{abuseResponseId}/actions/restore",
        {
          params: { path: { projectId, abuseResponseId } },
          body: { reviewedVersion, reason },
        },
      ),
    );
  }

  async listOperatorSupportSessions(projectId: string, limit = 25): Promise<SupportSession[]> {
    return unwrap(
      await this.#client.GET("/v1/operator/projects/{projectId}/support-sessions", {
        params: { path: { projectId }, query: { limit } },
      }),
    ).items;
  }

  async listCurrentOperatorSupportSessions(limit = 25): Promise<SupportSession[]> {
    return unwrap(
      await this.#client.GET("/v1/operator/support-sessions/current", {
        params: { query: { limit } },
      }),
    ).items;
  }

  async getOperatorSecurityInventory(limit = 50): Promise<OperatorSecurityInventory> {
    return unwrap(
      await this.#client.GET("/v1/operator/security", { params: { query: { limit } } }),
    );
  }

  async revokeOperatorIdentitySessions(
    developerIdentityId: string,
    reason: string,
  ): Promise<{ readonly revokedSessions: number }> {
    return unwrap(
      await this.#client.POST(
        "/v1/operator/security/identities/{developerIdentityId}/sessions/actions/revoke",
        { params: { path: { developerIdentityId } }, body: { reason } },
      ),
    );
  }

  async planOperatorEntitlementChange(
    input: OperatorEntitlementChangeInput,
  ): Promise<OperatorEntitlementChangePlan> {
    return unwrap(
      await this.#client.POST("/v1/operator/security/entitlements/actions/plan", {
        body: input,
      }),
    );
  }

  async applyOperatorEntitlementChange(
    input: OperatorEntitlementChangeInput,
    typedConfirmation: string,
  ): Promise<OperatorEntitlementChangeResult> {
    return unwrap(
      await this.#client.POST("/v1/operator/security/entitlements/actions/apply", {
        body: { input, typedConfirmation },
      }),
    );
  }

  async getOperatorProject(projectId: string): Promise<OperatorProjectView> {
    return unwrap(
      await this.#client.GET("/v1/operator/projects/{projectId}", {
        params: { path: { projectId } },
      }),
    );
  }

  async repairOperatorProvisioning(
    projectId: string,
    provisioningWorkflowId: string,
    input: {
      readonly reason: string;
      readonly action: "requeue" | "retry_compensation";
      readonly reviewedAt?: string;
      readonly operationKey?: string;
    },
  ): Promise<ProvisioningWorkflow> {
    return unwrap(
      await this.#client.POST(
        "/v1/operator/projects/{projectId}/provisioning/{provisioningWorkflowId}/actions/repair",
        {
          params: { path: { projectId, provisioningWorkflowId } },
          body: input,
        },
      ),
    );
  }

  async createOperatorQuotaOverride(
    projectId: string,
    input: CreateQuotaOverrideRequest,
  ): Promise<QuotaOverride> {
    return unwrap(
      await this.#client.POST("/v1/operator/projects/{projectId}/quota-overrides", {
        params: { path: { projectId } },
        body: input,
      }),
    );
  }

  async createOperatorAbuseResponse(
    projectId: string,
    input: CreateAbuseResponseRequest,
  ): Promise<AbuseResponse> {
    return unwrap(
      await this.#client.POST("/v1/operator/projects/{projectId}/abuse-responses", {
        params: { path: { projectId } },
        body: input,
      }),
    );
  }

  async createSupportSession(
    projectId: string,
    input: {
      readonly id: string;
      readonly environmentId: string | null;
      readonly permissions: SupportPermission[];
      readonly reason: string;
      readonly expiresAt: string;
    },
  ): Promise<SupportSession> {
    return unwrap(
      await this.#client.POST("/v1/operator/projects/{projectId}/support-sessions", {
        params: { path: { projectId } },
        body: input,
      }),
    );
  }

  async revokeSupportSession(
    projectId: string,
    supportSessionId: string,
    reason: string,
  ): Promise<SupportSession> {
    return unwrap(
      await this.#client.POST(
        "/v1/operator/projects/{projectId}/support-sessions/{supportSessionId}/actions/revoke",
        {
          params: { path: { projectId, supportSessionId } },
          body: { reason },
        },
      ),
    );
  }

  async listDeveloperWaitList(
    input: { readonly cursor?: string; readonly limit?: number } = {},
  ): Promise<DeveloperApplicantPage> {
    return unwrap(
      await this.#client.GET("/v1/operator/developer-waitlist", {
        params: { query: input },
      }),
    );
  }

  async getDeveloperWaitListApplicant(developerIdentityId: string): Promise<DeveloperApplicant> {
    return unwrap(
      await this.#client.GET("/v1/operator/developer-waitlist/{developerIdentityId}", {
        params: { path: { developerIdentityId } },
      }),
    );
  }

  async approveDeveloperWaitListApplicant(
    developerIdentityId: string,
    reason: string | null | undefined,
    idempotencyKey: string,
  ): Promise<DeveloperApplicant> {
    const normalizedReason = reason?.trim();
    return unwrap(
      await this.#client.POST(
        "/v1/operator/developer-waitlist/{developerIdentityId}/actions/approve",
        {
          params: {
            path: { developerIdentityId },
            header: { "Idempotency-Key": idempotencyKey },
          },
          body: normalizedReason ? { reason: normalizedReason } : {},
        },
      ),
    );
  }

  async rejectDeveloperWaitListApplicant(
    developerIdentityId: string,
    reason: string | null | undefined,
    idempotencyKey: string,
  ): Promise<DeveloperApplicant> {
    const normalizedReason = reason?.trim();
    return unwrap(
      await this.#client.POST(
        "/v1/operator/developer-waitlist/{developerIdentityId}/actions/reject",
        {
          params: {
            path: { developerIdentityId },
            header: { "Idempotency-Key": idempotencyKey },
          },
          body: normalizedReason ? { reason: normalizedReason } : {},
        },
      ),
    );
  }
}

export function createOperatorClient(options: OperatorClientOptions): MakoOperatorClient {
  return new MakoOperatorClient(options);
}

function unwrap<T>(result: {
  readonly data?: T;
  readonly error?: unknown;
  readonly response: Response;
}): T {
  if (result.data !== undefined) {
    return result.data;
  }
  throw responseError(result.error, result.response);
}

function expectNoContent(result: { readonly error?: unknown; readonly response: Response }): void {
  if (result.response.ok) {
    return;
  }
  throw responseError(result.error, result.response);
}

function validateAccessToken(token: string): string {
  if (
    token.length < 16 ||
    token.length > 16 * 1024 ||
    Array.from(token).some((character) => /\s/u.test(character) || isControl(character))
  ) {
    throw new TypeError("developer access token is invalid");
  }
  return token;
}

function explorerHeaders(capability: string): HeadersInit {
  if (
    capability.length < 32 ||
    capability.length > 16 * 1024 ||
    !capability.startsWith("mx1_") ||
    Array.from(capability).some((character) => /\s/u.test(character) || isControl(character))
  ) {
    throw new TypeError("explorer capability is invalid");
  }
  return { "X-Mako-Explorer-Capability": capability };
}

function responseError(error: unknown, response: Response): ManagementApiError {
  if (isApiErrorEnvelope(error)) {
    return new ManagementApiError(error.error, response.status);
  }
  return new ManagementApiError(
    {
      code: response.status >= 500 ? "unavailable" : "invalid_request",
      message: "management request failed",
      requestId: safeRequestId(response.headers.get("x-request-id")),
      retry: { kind: response.status >= 500 ? "immediate" : "never" },
    },
    response.status,
  );
}

function normalizeEndpoint(value: string): string {
  const endpoint = new URL(value);
  const isLocal =
    endpoint.protocol === "http:" &&
    (endpoint.hostname === "localhost" || endpoint.hostname === "127.0.0.1");
  if (endpoint.protocol !== "https:" && !isLocal) {
    throw new TypeError("management endpoint must use HTTPS outside local development");
  }
  endpoint.pathname = endpoint.pathname.replace(/\/+$/u, "");
  endpoint.search = "";
  endpoint.hash = "";
  return endpoint.toString();
}

function safeRequestId(value: string | null): string {
  return value !== null && value.length <= 128 ? value : "req_unknown";
}

function isControl(value: string): boolean {
  const codePoint = value.codePointAt(0);
  return codePoint !== undefined && (codePoint <= 0x1f || codePoint === 0x7f);
}
