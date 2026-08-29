/**
 * Pins `src/wire.ts` against the generated OpenAPI types.
 *
 * `@mako-cloud/rxdb` ships to npm on its own and must not depend on
 * `@mako-cloud/api-types`: that package is unpublished, and its `components`
 * tree carries every management, operator, and service schema the platform
 * defines. So the client declares the handful of application-facing payloads it
 * exchanges by hand, and this file — compiled only inside the monorepo, never
 * built into `dist` and never packed — proves each hand-written declaration
 * still describes the same values as its generated counterpart.
 *
 * A contract change that would break an application therefore fails here, in
 * this repository's `typecheck`, rather than in a customer's build.
 */

import type { components } from "@mako-cloud/api-types";

import type {
  ApiError,
  ApiErrorEnvelope,
  CollectionId,
  EnvironmentId,
  ErrorCode,
  MakoAuthSession,
  MakoAuthUser,
  MakoJsonDocument,
  MakoLiveStreamEvent,
  MakoMagicLinkAccepted,
  MakoProviderSignInStart,
  MakoPullResponse,
  MakoPushOutcome,
  MakoPushResponse,
  MakoPushRow,
  MakoResyncReason,
  MakoSignUpAccepted,
  MakoStorageObjectPage,
  MakoStorageObjectRecord,
  ProjectId,
  RetryAdvice,
  SafeDetail,
} from "../src/wire.js";

type Schemas = components["schemas"];

/** True when every value of `Source` is also a value of `Target`. */
type Assignable<Source, Target> = [Source] extends [Target] ? true : false;

/** True when the two types accept exactly the same values. */
type Mutual<Left, Right> = Assignable<Left, Right> extends true ? Assignable<Right, Left> : false;

/**
 * True when the two types declare exactly the same property names. Structural
 * assignability alone tolerates an extra optional property on either side;
 * this catches one that the contract does not define, or has stopped defining.
 */
type SameKeys<Left, Right> = Mutual<keyof Left, keyof Right>;

/* -------------------------------------------------------------------------- */
/* Scope identifiers                                                          */
/* -------------------------------------------------------------------------- */

// The client brands these so a raw string cannot be passed where a checked
// identifier is required, which is why only this direction holds.
const _projectId: Assignable<ProjectId, Schemas["ProjectId"]> = true;
const _environmentId: Assignable<EnvironmentId, Schemas["EnvironmentId"]> = true;
const _collectionId: Assignable<CollectionId, Schemas["CollectionId"]> = true;

/* -------------------------------------------------------------------------- */
/* Error envelope                                                             */
/* -------------------------------------------------------------------------- */

/**
 * `operator_step_up_required` is raised only on `/v1/operator/`, which no
 * application-user session or public project key can reach. Every other code
 * the contract defines must be representable by the client, so adding one to
 * the OpenAPI document without adding it to `ERROR_CODES` fails here.
 */
const _errorCodeIsComplete: Assignable<
  Exclude<Schemas["ErrorCode"], "operator_step_up_required">,
  ErrorCode
> = true;
const _errorCodeIsOnContract: Assignable<ErrorCode, Schemas["ErrorCode"]> = true;

const _retryAdvice: Mutual<RetryAdvice, Schemas["RetryAdvice"]> = true;

// The client hands error details out read-only, so the generated `string[]`
// widens to `readonly string[]` in one direction only. The second assertion
// still refuses a member the contract does not define.
const _safeDetailFromWire: Assignable<Schemas["SafeDetail"], SafeDetail> = true;
const _safeDetailIsOnContract: Assignable<SafeDetail, Schemas["SafeDetail"] | readonly string[]> =
  true;

const _apiErrorKeys: SameKeys<ApiError, Schemas["ApiError"]> = true;
// `code` and `details` are pinned above; the rest must match exactly.
const _apiErrorRest: Mutual<
  Omit<ApiError, "code" | "details">,
  Omit<Schemas["ApiError"], "code" | "details">
> = true;
const _apiErrorDetailsFromWire: Assignable<Schemas["ApiError"]["details"], ApiError["details"]> =
  true;

const _envelopeKeys: SameKeys<ApiErrorEnvelope, Schemas["ApiErrorEnvelope"]> = true;
const _envelopeVersion: Mutual<
  ApiErrorEnvelope["apiVersion"],
  Schemas["ApiErrorEnvelope"]["apiVersion"]
> = true;

/* -------------------------------------------------------------------------- */
/* Project authentication payloads                                            */
/* -------------------------------------------------------------------------- */

const _authUserKeys: SameKeys<MakoAuthUser, Schemas["AuthUser"]> = true;
const _authUser: Mutual<MakoAuthUser, Schemas["AuthUser"]> = true;

const _authSessionKeys: SameKeys<MakoAuthSession, Schemas["AuthSession"]> = true;
const _authSession: Mutual<MakoAuthSession, Schemas["AuthSession"]> = true;

const _signUpAcceptedKeys: SameKeys<MakoSignUpAccepted, Schemas["SignUpAccepted"]> = true;
const _signUpAccepted: Mutual<MakoSignUpAccepted, Schemas["SignUpAccepted"]> = true;

const _providerSignInStartKeys: SameKeys<MakoProviderSignInStart, Schemas["ProviderSignInStart"]> =
  true;
const _providerSignInStart: Mutual<MakoProviderSignInStart, Schemas["ProviderSignInStart"]> = true;

const _magicLinkAcceptedKeys: SameKeys<MakoMagicLinkAccepted, Schemas["MagicLinkAccepted"]> = true;
const _magicLinkAccepted: Mutual<MakoMagicLinkAccepted, Schemas["MagicLinkAccepted"]> = true;

/* -------------------------------------------------------------------------- */
/* Storage object payloads                                                    */
/* -------------------------------------------------------------------------- */

const _objectRecordKeys: SameKeys<MakoStorageObjectRecord, Schemas["ApplicationObject"]> = true;
const _objectRecord: Mutual<MakoStorageObjectRecord, Schemas["ApplicationObject"]> = true;

const _objectPageKeys: SameKeys<MakoStorageObjectPage, Schemas["ApplicationObjectPage"]> = true;
const _objectPage: Mutual<MakoStorageObjectPage, Schemas["ApplicationObjectPage"]> = true;

/* -------------------------------------------------------------------------- */
/* Replication payloads                                                       */
/* -------------------------------------------------------------------------- */

const _jsonDocument: Mutual<MakoJsonDocument, Schemas["JsonDocument"]> = true;

const _pullResponseKeys: SameKeys<MakoPullResponse, Schemas["PullResponse"]> = true;
const _pullResponse: Mutual<MakoPullResponse, Schemas["PullResponse"]> = true;

const _pushRowKeys: SameKeys<MakoPushRow, Schemas["PushRow"]> = true;
const _pushRow: Mutual<MakoPushRow, Schemas["PushRow"]> = true;

const _pushOutcomeKeys: SameKeys<MakoPushOutcome, Schemas["PushOutcome"]> = true;
// `error` carries the shared envelope, pinned above.
const _pushOutcomeRest: Mutual<
  Omit<MakoPushOutcome, "error">,
  Omit<Schemas["PushOutcome"], "error">
> = true;

const _pushResponseKeys: SameKeys<MakoPushResponse, Schemas["PushResponse"]> = true;
const _pushOutcomeIsTheOnlyMember: Mutual<
  MakoPushResponse["outcomes"][number] extends MakoPushOutcome ? true : false,
  true
> = true;

const _liveStreamEvent: Mutual<MakoLiveStreamEvent, Schemas["LiveStreamEvent"]> = true;
const _resyncReason: Mutual<
  MakoResyncReason,
  Extract<Schemas["LiveStreamEvent"], { event: "resync" }>["data"]["reason"]
> = true;
