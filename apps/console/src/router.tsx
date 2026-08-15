import { type ReactNode, useCallback, useEffect, useState } from "react";

import { useDeveloperAuth } from "./auth.js";
import { HostedSignInView, WaitListStatusView } from "./developer-auth-views.js";

export type ConsoleRoute =
  | { readonly name: "home" }
  | { readonly name: "login" }
  | { readonly name: "create_account" }
  | { readonly name: "check_email" }
  | { readonly name: "verify_email" }
  | { readonly name: "forgot_password" }
  | { readonly name: "check_recovery_email" }
  | { readonly name: "reset_password" }
  | { readonly name: "wait_list" }
  | {
      readonly name: "operator";
      readonly section:
        | "overview"
        | "tenants"
        | "tenant"
        | "operations"
        | "incidents"
        | "backups"
        | "sync"
        | "fleet"
        | "storage"
        | "security"
        | "activity"
        | "waitlist";
      readonly projectId?: string;
    }
  | { readonly name: "organization"; readonly organizationId: string }
  | { readonly name: "invitation"; readonly invitationId: string }
  | { readonly name: "project"; readonly projectId: string }
  | {
      readonly name: "environment_workspace";
      readonly projectId: string;
      readonly environmentId: string;
      readonly section:
        | "overview"
        | "data"
        | "sync"
        | "policies"
        | "backups"
        | "connect"
        | "settings";
    }
  | {
      readonly name: "collections";
      readonly projectId: string;
      readonly environmentId: string;
    }
  | {
      readonly name: "collection";
      readonly projectId: string;
      readonly environmentId: string;
      readonly collectionId: string;
    }
  | {
      readonly name: "policy";
      readonly projectId: string;
      readonly environmentId: string;
      readonly collectionId: string;
    }
  | {
      readonly name: "users";
      readonly projectId: string;
      readonly environmentId: string;
    }
  | {
      readonly name: "user";
      readonly projectId: string;
      readonly environmentId: string;
      readonly userId: string;
    }
  | {
      readonly name: "credentials";
      readonly projectId: string;
      readonly environmentId: string;
    }
  | {
      readonly name: "functions";
      readonly projectId: string;
      readonly environmentId: string;
    }
  | {
      readonly name: "function";
      readonly projectId: string;
      readonly environmentId: string;
      readonly functionName: string;
    }
  | {
      readonly name: "observability";
      readonly projectId: string;
      readonly environmentId: string;
    }
  | { readonly name: "not_found"; readonly path: string };

export function matchConsoleRoute(pathname: string): ConsoleRoute {
  const normalized = normalizePath(pathname);
  if (normalized === "/") {
    return { name: "home" };
  }
  if (normalized === "/login" || normalized === "/sign-in") {
    return { name: "login" };
  }
  if (normalized === "/create-account") return { name: "create_account" };
  if (normalized === "/check-email") return { name: "check_email" };
  if (normalized === "/verify-email") return { name: "verify_email" };
  if (normalized === "/forgot-password") return { name: "forgot_password" };
  if (normalized === "/check-recovery-email") return { name: "check_recovery_email" };
  if (normalized === "/reset-password") return { name: "reset_password" };
  if (normalized === "/wait-list") return { name: "wait_list" };
  if (normalized === "/operator" || normalized === "/operator/overview") {
    return { name: "operator", section: "overview" };
  }
  const operatorTenant = normalized.match(/^\/operator\/tenants\/(prj_[A-Za-z0-9_-]{8,64})$/u);
  if (operatorTenant?.[1] !== undefined) {
    return { name: "operator", section: "tenant", projectId: operatorTenant[1] };
  }
  const operatorSection = normalized.match(
    /^\/operator\/(tenants|operations|incidents|backups|sync|fleet|storage|security|activity|waitlist)$/u,
  )?.[1];
  if (operatorSection !== undefined) {
    return {
      name: "operator",
      section: operatorSection as Exclude<
        Extract<ConsoleRoute, { readonly name: "operator" }>["section"],
        "overview" | "tenant"
      >,
    };
  }
  const organization = normalized.match(/^\/organizations\/(org_[A-Za-z0-9_-]{8,64})$/u);
  if (organization?.[1] !== undefined) {
    return { name: "organization", organizationId: organization[1] };
  }
  const invitation = normalized.match(/^\/invitations\/(inv_[A-Za-z0-9_-]{8,64})$/u);
  if (invitation?.[1] !== undefined) {
    return { name: "invitation", invitationId: invitation[1] };
  }
  const project = normalized.match(/^\/projects\/(prj_[A-Za-z0-9_-]{8,64})$/u);
  if (project?.[1] !== undefined) {
    return { name: "project", projectId: project[1] };
  }
  const environmentWorkspace = normalized.match(
    /^\/projects\/(prj_[A-Za-z0-9_-]{8,64})\/environments\/(env_[A-Za-z0-9_-]{8,64})(?:\/(overview|data|sync|policies|backups|connect|settings))?$/u,
  );
  if (environmentWorkspace?.[1] !== undefined && environmentWorkspace[2] !== undefined) {
    return {
      name: "environment_workspace",
      projectId: environmentWorkspace[1],
      environmentId: environmentWorkspace[2],
      section: (environmentWorkspace[3] ?? "overview") as Extract<
        ConsoleRoute,
        { readonly name: "environment_workspace" }
      >["section"],
    };
  }
  const policy = normalized.match(
    /^\/projects\/(prj_[A-Za-z0-9_-]{8,64})\/environments\/(env_[A-Za-z0-9_-]{8,64})\/collections\/([a-z][a-z0-9_-]{0,62})\/policies$/u,
  );
  if (policy?.[1] !== undefined && policy[2] !== undefined && policy[3] !== undefined) {
    return {
      name: "policy",
      projectId: policy[1],
      environmentId: policy[2],
      collectionId: policy[3],
    };
  }
  const collection = normalized.match(
    /^\/projects\/(prj_[A-Za-z0-9_-]{8,64})\/environments\/(env_[A-Za-z0-9_-]{8,64})\/collections\/([a-z][a-z0-9_-]{0,62})$/u,
  );
  if (collection?.[1] !== undefined && collection[2] !== undefined && collection[3] !== undefined) {
    return {
      name: "collection",
      projectId: collection[1],
      environmentId: collection[2],
      collectionId: collection[3],
    };
  }
  const collections = normalized.match(
    /^\/projects\/(prj_[A-Za-z0-9_-]{8,64})\/environments\/(env_[A-Za-z0-9_-]{8,64})\/collections$/u,
  );
  if (collections?.[1] !== undefined && collections[2] !== undefined) {
    return {
      name: "collections",
      projectId: collections[1],
      environmentId: collections[2],
    };
  }
  const user = normalized.match(
    /^\/projects\/(prj_[A-Za-z0-9_-]{8,64})\/environments\/(env_[A-Za-z0-9_-]{8,64})\/users\/(usr_[A-Za-z0-9_-]{8,96})$/u,
  );
  if (user?.[1] !== undefined && user[2] !== undefined && user[3] !== undefined) {
    return {
      name: "user",
      projectId: user[1],
      environmentId: user[2],
      userId: user[3],
    };
  }
  const users = normalized.match(
    /^\/projects\/(prj_[A-Za-z0-9_-]{8,64})\/environments\/(env_[A-Za-z0-9_-]{8,64})\/users$/u,
  );
  if (users?.[1] !== undefined && users[2] !== undefined) {
    return { name: "users", projectId: users[1], environmentId: users[2] };
  }
  const credentials = normalized.match(
    /^\/projects\/(prj_[A-Za-z0-9_-]{8,64})\/environments\/(env_[A-Za-z0-9_-]{8,64})\/credentials$/u,
  );
  if (credentials?.[1] !== undefined && credentials[2] !== undefined) {
    return {
      name: "credentials",
      projectId: credentials[1],
      environmentId: credentials[2],
    };
  }
  const edgeFunction = normalized.match(
    /^\/projects\/(prj_[A-Za-z0-9_-]{8,64})\/environments\/(env_[A-Za-z0-9_-]{8,64})\/functions\/([a-z][a-z0-9-]{0,62})$/u,
  );
  if (
    edgeFunction?.[1] !== undefined &&
    edgeFunction[2] !== undefined &&
    edgeFunction[3] !== undefined
  ) {
    return {
      name: "function",
      projectId: edgeFunction[1],
      environmentId: edgeFunction[2],
      functionName: edgeFunction[3],
    };
  }
  const functions = normalized.match(
    /^\/projects\/(prj_[A-Za-z0-9_-]{8,64})\/environments\/(env_[A-Za-z0-9_-]{8,64})\/functions$/u,
  );
  if (functions?.[1] !== undefined && functions[2] !== undefined) {
    return {
      name: "functions",
      projectId: functions[1],
      environmentId: functions[2],
    };
  }
  const observability = normalized.match(
    /^\/projects\/(prj_[A-Za-z0-9_-]{8,64})\/environments\/(env_[A-Za-z0-9_-]{8,64})\/observability$/u,
  );
  if (observability?.[1] !== undefined && observability[2] !== undefined) {
    return {
      name: "observability",
      projectId: observability[1],
      environmentId: observability[2],
    };
  }
  return { name: "not_found", path: normalized };
}

/**
 * Map pre-workspace saved links to the nested environment hierarchy. Only
 * validated resource identifiers are copied; query strings and fragments are
 * deliberately excluded so secrets, reasons, emails, or document content can
 * never be carried into the replacement URL.
 */
export function legacyConsoleRedirectPath(pathname: string): string | null {
  const match = normalizePath(pathname).match(
    /^\/projects\/(prj_[A-Za-z0-9_-]{8,64})\/environments\/(env_[A-Za-z0-9_-]{8,64})\/(explorer|api|replication|recovery)$/u,
  );
  if (match?.[1] === undefined || match[2] === undefined || match[3] === undefined) return null;
  const section = {
    explorer: "data",
    api: "connect",
    replication: "sync",
    recovery: "backups",
  }[match[3]];
  return `/projects/${match[1]}/environments/${match[2]}/${section}`;
}

export function useConsoleRoute(): {
  readonly route: ConsoleRoute;
  readonly navigate: (path: string, replace?: boolean) => void;
} {
  const [route, setRoute] = useState(() => {
    const redirect = legacyConsoleRedirectPath(window.location.pathname);
    if (redirect !== null) window.history.replaceState(null, "", redirect);
    return matchConsoleRoute(redirect ?? window.location.pathname);
  });
  useEffect(() => {
    const onPopState = () => {
      const redirect = legacyConsoleRedirectPath(window.location.pathname);
      if (redirect !== null) window.history.replaceState(null, "", redirect);
      setRoute(matchConsoleRoute(redirect ?? window.location.pathname));
    };
    window.addEventListener("popstate", onPopState);
    return () => window.removeEventListener("popstate", onPopState);
  }, []);
  const navigate = useCallback((path: string, replace = false) => {
    const normalized = legacyConsoleRedirectPath(path) ?? normalizePath(path);
    if (replace) {
      window.history.replaceState(null, "", normalized);
    } else {
      window.history.pushState(null, "", normalized);
    }
    setRoute(matchConsoleRoute(normalized));
  }, []);
  return { route, navigate };
}

export function RequireDeveloperSession({ children }: { readonly children: ReactNode }) {
  const { state, signIn } = useDeveloperAuth();
  if (state.status === "loading") {
    return <ConsoleStatus label="Checking your developer session…" />;
  }
  if (state.status === "error") {
    return (
      <main className="centered">
        <section className="panel" role="alert">
          <h1>Authentication unavailable</h1>
          <p>{state.message}</p>
          <button type="button" onClick={() => void signIn()}>
            Try sign-in again
          </button>
        </section>
      </main>
    );
  }
  if (state.status === "anonymous") {
    return <SignInView />;
  }
  if (state.session.audience === "mako-developer-waitlist") {
    return <WaitListStatusView />;
  }
  return children;
}

export function SignInView({
  navigate = (path: string) => window.location.assign(path),
}: {
  readonly navigate?: (path: string, replace?: boolean) => void;
} = {}) {
  const { acceptsSessionToken, selfService, signIn } = useDeveloperAuth();
  const [pending, setPending] = useState(false);
  const [failed, setFailed] = useState(false);
  const [sessionToken, setSessionToken] = useState("");
  const begin = async (token?: string) => {
    setPending(true);
    setFailed(false);
    try {
      await signIn(token);
    } catch {
      setFailed(true);
      setPending(false);
    }
  };
  if (selfService !== null) {
    return <HostedSignInView navigate={navigate} />;
  }
  return (
    <main className="centered">
      <section className="panel sign-in" aria-labelledby="sign-in-title">
        <p className="eyebrow">Mako Cloud</p>
        <h1 id="sign-in-title">Sign in to your developer account</h1>
        <p>Your application users and developer identity remain separate.</p>
        {failed ? (
          <p role="alert">
            {acceptsSessionToken
              ? "The developer session token is invalid, expired, or for another environment."
              : "Sign-in could not be started. Please try again."}
          </p>
        ) : null}
        {acceptsSessionToken ? (
          <form
            onSubmit={(event) => {
              event.preventDefault();
              void begin(sessionToken);
            }}
          >
            <label>
              Short-lived developer session token
              <input
                type="password"
                name="developer-session-token"
                value={sessionToken}
                autoComplete="off"
                spellCheck={false}
                required
                onChange={(event) => setSessionToken(event.currentTarget.value)}
              />
            </label>
            <p>The token stays only in this browser tab and expires within one hour.</p>
            <button type="submit" disabled={pending || sessionToken.trim() === ""}>
              {pending ? "Verifying session…" : "Sign in with session token"}
            </button>
          </form>
        ) : (
          <button type="button" disabled={pending} onClick={() => void begin()}>
            {pending ? "Opening identity provider…" : "Continue to sign in"}
          </button>
        )}
      </section>
    </main>
  );
}

function ConsoleStatus({ label }: { readonly label: string }) {
  return (
    <main className="centered" aria-live="polite" aria-busy="true">
      <p>{label}</p>
    </main>
  );
}

function normalizePath(path: string): string {
  if (!path.startsWith("/") || path.startsWith("//")) {
    return "/not-found";
  }
  const withoutQuery = path.split(/[?#]/u, 1)[0] ?? "/";
  const normalized = withoutQuery.replace(/\/{2,}/gu, "/").replace(/\/$/u, "");
  return normalized === "" ? "/" : normalized;
}
