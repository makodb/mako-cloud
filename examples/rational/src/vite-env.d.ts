/// <reference types="vite/client" />

/** Injected by vite.config.ts from mako.env.json; null when no tenant is configured. */
declare const __RATIONAL_ENV__: {
  readonly endpoint: string;
  readonly projectId: string;
  readonly environmentId: string;
  readonly publicProjectKey: string;
  readonly functionsEndpoint?: string | null;
  readonly signIn?: import("./config.js").SignInSettings;
} | null;
