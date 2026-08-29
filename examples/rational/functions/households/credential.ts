/**
 * Where this function's scoped service credential comes from.
 *
 * The platform generates the value of a function secret and the secret of a
 * service credential, and neither can be created with a value the developer
 * chooses, so there is no way to install a service credential as a function
 * secret — the pattern Mako Cloud's edge-function guide documents. Until
 * there is, `scripts/bootstrap.mjs --functions` replaces this module in the
 * copy of the directory it uploads, and the deployed function reads the
 * credential from there.
 *
 * The secret named below is still attached to the deployment: the day a
 * function secret can carry a chosen value, the bootstrap stops rewriting
 * this file and this function keeps working unchanged.
 */
declare const Deno: { readonly env: { get(name: string): string | undefined } };

export const SERVICE_CREDENTIAL_SECRET = "HOUSEHOLDS_SERVICE_KEY" as const;

/** The credential the deployment was given, from the secret when it holds one. */
export function serviceCredential(): string {
  const injected = Deno.env.get(SERVICE_CREDENTIAL_SECRET);
  if (injected?.startsWith("mako_sk.") === true) return injected;
  return bundled();
}

/**
 * Replaced at deploy time. Running unbundled — `mako functions serve` without
 * the bootstrap — there is nothing to return, and the function fails closed.
 */
function bundled(): string {
  throw new Error(
    "this deployment carries no households service credential; deploy it with scripts/bootstrap.mjs --functions",
  );
}
