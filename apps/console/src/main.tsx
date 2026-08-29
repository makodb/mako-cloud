import { createRoot } from "react-dom/client";

import type { ConsoleBootstrapOptions } from "./bootstrap.js";
import { mountConsole } from "./bootstrap.js";
import { HostedDeveloperAuthAdapter } from "./hosted-auth.js";
import { HostedOperatorAuthAdapter } from "./hosted-operator-auth.js";
import "./styles.css";
import "./home.css";
import "./project-home.css";
import "./surfaced.css";
import "./usage-activity.css";
import "./storage.css";
import "./webhooks.css";
import "./auth.css";

declare global {
  interface Window {
    __MAKO_CONSOLE__?: ConsoleBootstrapOptions;
  }
}

const rootElement = document.querySelector("#root");
if (rootElement === null) {
  throw new Error("Mako console root element is missing");
}

const configuration = window.__MAKO_CONSOLE__ ?? hostedConfiguration();
if (configuration === undefined) {
  createRoot(rootElement).render(
    <main className="centered">
      <section className="panel" role="alert">
        <p className="eyebrow">Setup required</p>
        <h1>Console configuration is missing.</h1>
        <p>The host must supply a management endpoint and developer authentication adapter.</p>
      </section>
    </main>,
  );
} else {
  mountConsole(rootElement, configuration);
}

function hostedConfiguration(): ConsoleBootstrapOptions | undefined {
  const configured = document
    .querySelector<HTMLMetaElement>('meta[name="mako-console-management-endpoint"]')
    ?.content.trim();
  if (configured === undefined || configured === "") return undefined;
  const managementEndpoint = configured === "same-origin" ? window.location.origin : configured;
  const featureEnabled = (name: string) =>
    document.querySelector<HTMLMetaElement>(`meta[name="${name}"]`)?.content.trim() === "enabled";
  return {
    managementEndpoint,
    developerAuth: new HostedDeveloperAuthAdapter({
      managementEndpoint,
      storage: window.sessionStorage,
    }),
    operatorAuth: new HostedOperatorAuthAdapter({
      managementEndpoint,
    }),
    developerWorkspaceEnabled: featureEnabled("mako-console-developer-workspace"),
    developerExplorerAdminEnabled: featureEnabled("mako-console-developer-explorer-admin"),
    developerDataJobsEnabled: featureEnabled("mako-console-developer-data-jobs"),
    developerSyncDetailsEnabled: featureEnabled("mako-console-developer-sync-details"),
    developerRestoreEnabled: featureEnabled("mako-console-developer-restore"),
  };
}
