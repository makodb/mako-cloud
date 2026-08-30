/**
 * The scoped service credential this function runs under, installed as a
 * function secret with its value supplied by `scripts/bootstrap.mjs
 * --functions`. The uploaded bundle carries no credential.
 */
declare const Deno: { readonly env: { get(name: string): string | undefined } };

export const SERVICE_CREDENTIAL_SECRET = "NIGHTLY_SERVICE_KEY" as const;

export function serviceCredential(): string {
  const injected = Deno.env.get(SERVICE_CREDENTIAL_SECRET);
  if (injected?.startsWith("mako_sk.") === true) return injected;
  throw new Error(
    `this deployment has no ${SERVICE_CREDENTIAL_SECRET}; deploy it with scripts/bootstrap.mjs --functions`,
  );
}
