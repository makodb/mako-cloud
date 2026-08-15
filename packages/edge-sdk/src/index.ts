/** Caller-aware SDK for Mako Cloud edge functions. */

export {
  MakoCallerIdentityRequiredError,
  MakoEdgeSdkError,
  createFunctionClient,
  createFunctionClientFromRequest,
  createServiceClient,
  type FunctionAuthClient,
  type FunctionClient,
  type FunctionClientOptions,
  type FunctionDocument,
  type FunctionDocumentClient,
  type FunctionDocumentMutation,
  type FunctionDocumentMutationResult,
  type FunctionDocumentQuery,
  type FunctionDocumentQueryPage,
  type JsonObject,
  type RuntimeFunctionClientOptions,
  type ServiceFunctionClient,
  type ServiceFunctionClientOptions,
} from "./client.js";

export const PACKAGE_NAME = "@mako-cloud/edge-sdk" as const;
