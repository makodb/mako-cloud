/**
 * Where this function's scoped service credential comes from.
 *
 * A function secret can carry a value the developer chooses (findings log
 * #11), so `scripts/bootstrap.mjs --functions` issues the credential and
 * installs it under this name; the deployment reads it from the environment
 * like any other secret and the uploaded bundle carries no credential at all.
 */
declare const Deno: { readonly env: { get(name: string): string | undefined } };

export const SERVICE_CREDENTIAL_SECRET = "HOUSEHOLDS_SERVICE_KEY" as const;

/** The credential the deployment was given, or nothing to run on. */
export function serviceCredential(): string {
  const injected = Deno.env.get(SERVICE_CREDENTIAL_SECRET);
  if (injected?.startsWith("mako_sk.") === true) return injected;
  throw new Error(
    `this deployment has no ${SERVICE_CREDENTIAL_SECRET}; deploy it with scripts/bootstrap.mjs --functions`,
  );
}
