import { MakoAuthError, type MakoAuthClient } from "./auth.js";
import {
  authenticationRequiredError,
  replicationResponseError,
  sessionRefreshUnavailableError,
} from "./replication-error.js";
import { ERROR_CODES } from "./wire.js";

/**
 * The access token a renewal produced and the service then refused anyway.
 *
 * One entry per auth client, so every replication scope on the page learns from
 * the first scope that discovered the session is gone: the credential is dead,
 * and renewing again would only add a second request to every retry RxDB makes.
 * A later sign-in issues a different token, which no longer matches.
 */
const refusedAccessTokens = new WeakMap<MakoAuthClient, string>();

/** The current access token, with an auth failure mapped onto the replication error. */
export async function replicationAccessToken(auth: MakoAuthClient): Promise<string> {
  try {
    return await auth.validAccessToken();
  } catch (error) {
    throw replicationSessionError(error);
  }
}

/**
 * Send an authenticated replication request, renewing the session at most once.
 *
 * A `401` is ordinarily an expired access token, so the request is repeated
 * once with a renewed one -- `MakoAuthClient` coalesces renewals, so concurrent
 * scopes share a single token request. A second refusal is definitive: the
 * token is remembered as refused and the terminal `unauthenticated` error is
 * thrown without any further attempt.
 */
export async function sendReplicationRequest(
  auth: MakoAuthClient,
  accessToken: string,
  send: (accessToken: string) => Promise<Response>,
): Promise<Response> {
  const response = await send(accessToken);
  if (response.ok) {
    return response;
  }
  const error = await replicationResponseError(response);
  if (error.code !== ERROR_CODES.UNAUTHENTICATED || refusedAccessTokens.get(auth) === accessToken) {
    throw error;
  }
  let renewed: string;
  try {
    renewed = (await auth.refreshSession()).accessToken;
  } catch (refreshError) {
    throw replicationSessionError(refreshError);
  }
  const retried = await send(renewed);
  if (retried.ok) {
    return retried;
  }
  const retriedError = await replicationResponseError(retried);
  if (retriedError.code === ERROR_CODES.UNAUTHENTICATED) {
    refusedAccessTokens.set(auth, renewed);
  }
  throw retriedError;
}

/**
 * A session failure as replication must report it: a renewal that never reached
 * a verdict is retryable and keeps the session (see the session-renewal table
 * in `docs/user-book.md`); a definitive refusal is terminal.
 */
function replicationSessionError(error: unknown) {
  return error instanceof MakoAuthError && error.retryable
    ? sessionRefreshUnavailableError()
    : authenticationRequiredError();
}
