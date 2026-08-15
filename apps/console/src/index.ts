/** Developer and operator console application. */
import { MANAGEMENT_OPERATIONS, OPERATOR_OPERATIONS } from "@mako-cloud/management-sdk";

export const APP_NAME = "Mako Cloud Console" as const;

/** The console intentionally consumes the same operation inventory as the public SDK. */
export const CONSOLE_MANAGEMENT_OPERATIONS = MANAGEMENT_OPERATIONS;

/** The restricted operator route consumes the complete, separately authenticated operator API. */
export const CONSOLE_OPERATOR_OPERATIONS = OPERATOR_OPERATIONS;

export { ConsoleApp } from "./app.js";
export { ApplicationUserScreen, ApplicationUsersScreen } from "./application-users.js";
export { ApiFailureNotice, toConsoleApiFailure } from "./api-error.js";
export {
  DeveloperAuthProvider,
  MemoryDeveloperAuthAdapter,
  isSessionActive,
  useDeveloperAuth,
  type DeveloperAuthAdapter,
  type DeveloperAuthState,
  type DeveloperProfile,
  type DeveloperSelfServiceAdapter,
  type DeveloperSession,
} from "./auth.js";
export {
  CheckEmailView,
  CheckRecoveryEmailView,
  CreateAccountView,
  ForgotPasswordView,
  HostedSignInView,
  ResetPasswordView,
  VerifyEmailView,
  WaitListStatusView,
  consumeFragmentToken,
} from "./developer-auth-views.js";
export { mountConsole, type ConsoleBootstrapOptions } from "./bootstrap.js";
export {
  HostedDeveloperAuthAdapter,
  ShortLivedDeveloperSessionAuthAdapter,
  isSessionTokenDeveloperAuthAdapter,
  parseHostedDeveloperSession,
  parseShortLivedDeveloperSession,
  type HostedDeveloperAuthOptions,
  type SessionTokenDeveloperAuthAdapter,
  type ShortLivedDeveloperSessionAuthOptions,
} from "./hosted-auth.js";
export {
  HostedOperatorAuthAdapter,
  parseHostedOperatorSession,
  type HostedOperatorAuthOptions,
} from "./hosted-operator-auth.js";
export { ConsoleErrorBoundary, RequestId, requestIdFor } from "./error-boundary.js";
export { FunctionScreen, FunctionsScreen } from "./functions.js";
export {
  ObservabilityScreen,
  filterObservabilityRecords,
  observabilityRecordsToCsv,
} from "./observability.js";
export {
  MemoryOperatorAuthAdapter,
  OperatorAuthProvider,
  isOperatorSessionActive,
  useOperatorAuth,
  type OperatorAuthAdapter,
  type OperatorAuthState,
  type OperatorProfile,
  type OperatorSession,
} from "./operator-auth.js";
export { OperatorConsoleScreen, RequireOperatorSession } from "./operator.js";
export { OperatorWaitListPanel } from "./operator-waitlist.js";
export {
  OperatorManagementProvider,
  OperatorSessionExpiredError,
  useOperatorClient,
} from "./operator-management.js";
export { CollectionScreen, CollectionsScreen } from "./collections.js";
export { CredentialsScreen } from "./credentials.js";
export {
  DeveloperSessionExpiredError,
  ManagementProvider,
  useManagementClient,
} from "./management.js";
export {
  RequireDeveloperSession,
  SignInView,
  legacyConsoleRedirectPath,
  matchConsoleRoute,
  useConsoleRoute,
  type ConsoleRoute,
} from "./router.js";
export {
  InvitationAcceptScreen,
  OrganizationScreen,
  OrganizationsScreen,
} from "./organizations.js";
export { LifecycleBadge, ProjectScreen, ProjectsPanel } from "./projects.js";
export { PolicyScreen } from "./policies.js";
