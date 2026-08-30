/**
 * How Rational finds its tenant. In order: an explicit `window.__RATIONAL__`
 * (the test suites inject one), the `mako.env.json` the bootstrap wrote
 * (compiled in by Vite), and finally the in-browser fake backend so the app
 * runs with no server at all. A compiled-in file whose ids are placeholders
 * counts as no file: a checkout that has not been pointed at a project yet
 * runs on the fake backend rather than failing every request.
 */
export type RationalMode = "live" | "fake";

/** A sign-in provider the environment knows about, enabled or not. */
export interface SignInProviderSetting {
  readonly name: string;
  readonly enabled: boolean;
  /** What the button says; the provider's name when absent. */
  readonly label?: string;
}

/**
 * Which sign-in methods this environment offers, as the bootstrap read them
 * from `mako auth-settings get`. A method the environment does not enable is
 * shown and refused here rather than failing on use.
 */
export interface SignInSettings {
  readonly providers: readonly SignInProviderSetting[];
  readonly magicLinks: boolean;
}

export interface RationalConfig {
  readonly mode: RationalMode;
  readonly endpoint: string;
  readonly projectId: string;
  readonly environmentId: string;
  readonly publicProjectKey: string;
  /** Where the edge gateway serves this environment's functions, if deployed. */
  readonly functionsEndpoint: string | null;
  readonly signIn: SignInSettings;
  /** Milliseconds RxDB waits before retrying a failed pull or push. */
  readonly retryTimeMs: number;
  /** Start with the simulated network switched off (offline restart tests). */
  readonly startOffline: boolean;
}

export interface RationalOverrides {
  readonly mode?: RationalMode;
  readonly endpoint?: string;
  readonly projectId?: string;
  readonly environmentId?: string;
  readonly publicProjectKey?: string;
  readonly functionsEndpoint?: string | null;
  readonly signIn?: SignInSettings;
  readonly retryTimeMs?: number;
  readonly startOffline?: boolean;
}

declare global {
  interface Window {
    __RATIONAL__?: RationalOverrides;
  }
}

/**
 * What the app assumes when nothing told it otherwise: password only. An
 * environment that enables more says so in `mako.env.json`.
 */
export const DEFAULT_SIGN_IN: SignInSettings = { providers: [], magicLinks: false };

/**
 * The fake backend implements one enabled provider and one the environment
 * knows but has switched off, so both states have a screen to render.
 */
export const FAKE_CONFIG: RationalConfig = {
  mode: "fake",
  endpoint: "same-origin",
  projectId: "prj_rational0",
  environmentId: "env_rational0",
  publicProjectKey: "mako_pk.rational-fake",
  functionsEndpoint: "same-origin",
  signIn: {
    providers: [
      { name: "demo-idp", enabled: true, label: "Demo IdP" },
      { name: "example-sso", enabled: false, label: "Example SSO" },
    ],
    magicLinks: true,
  },
  retryTimeMs: 200,
  startOffline: false,
};

/**
 * A project id, environment id, or public key that names nothing — the values
 * the example configuration ships with, which a build that has no project of
 * its own is compiled against. Configuration that names no project means the
 * same thing as no configuration at all: run against the in-browser fake
 * backend, rather than fail every request against a project that is not there.
 */
const PLACEHOLDER = /(?:^|[._-])replace[_-]?me$/iu;

/** Whether a compiled-in configuration names a real tenant or a placeholder. */
export function isPlaceholderTenant(tenant: {
  readonly endpoint: string;
  readonly projectId: string;
  readonly environmentId: string;
  readonly publicProjectKey: string;
}): boolean {
  return (
    tenant.endpoint.trim() === "" ||
    PLACEHOLDER.test(tenant.projectId) ||
    PLACEHOLDER.test(tenant.environmentId) ||
    PLACEHOLDER.test(tenant.publicProjectKey)
  );
}

export function resolveConfig(overrides: RationalOverrides | undefined): RationalConfig {
  const compiled = __RATIONAL_ENV__;
  const tenant = compiled === null || isPlaceholderTenant(compiled) ? null : compiled;
  const base: RationalConfig =
    tenant === null
      ? FAKE_CONFIG
      : {
          mode: "live",
          endpoint: tenant.endpoint,
          projectId: tenant.projectId,
          environmentId: tenant.environmentId,
          publicProjectKey: tenant.publicProjectKey,
          functionsEndpoint: tenant.functionsEndpoint ?? null,
          signIn: tenant.signIn ?? DEFAULT_SIGN_IN,
          retryTimeMs: 2_000,
          startOffline: false,
        };
  const merged = { ...base, ...stripUndefined(overrides ?? {}) };
  return {
    ...merged,
    endpoint: absolute(merged.endpoint),
    functionsEndpoint:
      merged.functionsEndpoint === null ? null : absolute(merged.functionsEndpoint),
  };
}

/** The URL of the `households` function, or null when it is not deployed. */
export function functionUrl(config: RationalConfig, name: string): string | null {
  if (config.functionsEndpoint === null) return null;
  const base = config.functionsEndpoint.replace(/\/$/u, "");
  return `${base}/${config.projectId}--${config.environmentId}/functions/v1/${name}`;
}

/** Where a provider or a magic link may send the browser back. */
export function redirectUrl(): string {
  return `${window.location.origin}${window.location.pathname}`;
}

function absolute(endpoint: string): string {
  return endpoint === "same-origin" ? window.location.origin : endpoint;
}

function stripUndefined<T extends object>(value: T): Partial<T> {
  return Object.fromEntries(
    Object.entries(value).filter(([, entry]) => entry !== undefined),
  ) as Partial<T>;
}
