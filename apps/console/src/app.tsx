import { type ReactNode, useEffect } from "react";

import { ApplicationUserScreen, ApplicationUsersScreen } from "./application-users.js";
import { useDeveloperAuth } from "./auth.js";
import { CollectionScreen, CollectionsScreen } from "./collections.js";
import { CredentialsScreen } from "./credentials.js";
import { EnvironmentWorkspaceLayout, EnvironmentWorkspaceScreen } from "./developer-workspace.js";
import {
  CheckEmailView,
  CheckRecoveryEmailView,
  CreateAccountView,
  ForgotPasswordView,
  ResetPasswordView,
  VerifyEmailView,
  WaitListStatusView,
} from "./developer-auth-views.js";
import { ConsoleErrorBoundary } from "./error-boundary.js";
import { FunctionScreen, FunctionsScreen } from "./functions.js";
import { ObservabilityScreen } from "./observability.js";
import { RequireOperatorSession } from "./operator.js";
import { OperatorWorkspaceScreen } from "./operator-control-center.js";
import { InvitationAcceptScreen, TeamScreen, TeamsScreen } from "./teams.js";
import { ProjectScreen } from "./projects.js";
import { PolicyScreen } from "./policies.js";
import { RequireDeveloperSession, SignInView, useConsoleRoute } from "./router.js";

export function ConsoleApp({
  developerWorkspaceEnabled = true,
  developerExplorerAdminEnabled = true,
  developerDataJobsEnabled = true,
  developerSyncDetailsEnabled = true,
  developerRestoreEnabled = true,
}: {
  readonly developerWorkspaceEnabled?: boolean;
  readonly developerExplorerAdminEnabled?: boolean;
  readonly developerDataJobsEnabled?: boolean;
  readonly developerSyncDetailsEnabled?: boolean;
  readonly developerRestoreEnabled?: boolean;
}) {
  const { route, navigate } = useConsoleRoute();
  const focusPath = window.location.pathname;
  useEffect(() => {
    const frame = window.requestAnimationFrame(() => {
      document
        .querySelector<HTMLElement>(
          focusPath.startsWith("/operator") ? "#operator-main-content" : "#main-content",
        )
        ?.focus();
    });
    return () => window.cancelAnimationFrame(frame);
  }, [focusPath]);
  return (
    <ConsoleErrorBoundary>
      {route.name === "operator" ? (
        <RequireOperatorSession>
          <OperatorWorkspaceScreen
            section={route.section}
            projectId={route.projectId}
            navigate={navigate}
            onExit={() => navigate("/")}
          />
        </RequireOperatorSession>
      ) : route.name === "login" ? (
        <SignInView navigate={navigate} />
      ) : route.name === "create_account" ? (
        <CreateAccountView navigate={navigate} />
      ) : route.name === "check_email" ? (
        <CheckEmailView />
      ) : route.name === "verify_email" ? (
        <VerifyEmailView />
      ) : route.name === "forgot_password" ? (
        <ForgotPasswordView navigate={navigate} />
      ) : route.name === "check_recovery_email" ? (
        <CheckRecoveryEmailView />
      ) : route.name === "reset_password" ? (
        <ResetPasswordView />
      ) : route.name === "wait_list" ? (
        <RequireDeveloperSession>
          <WaitListStatusView />
        </RequireDeveloperSession>
      ) : (
        <RequireDeveloperSession>
          <AuthenticatedShell>
            {route.name === "home" ? (
              <TeamsScreen onOpen={(teamId) => navigate(`/teams/${teamId}`)} />
            ) : route.name === "team" ? (
              <TeamScreen
                teamId={route.teamId}
                onOpen={(teamId) => navigate(`/teams/${teamId}`)}
                onOpenProject={(projectId) => navigate(`/projects/${projectId}`)}
              />
            ) : route.name === "invitation" ? (
              <InvitationAcceptScreen
                invitationId={route.invitationId}
                onAccepted={(teamId) => navigate(`/teams/${teamId}`, true)}
              />
            ) : route.name === "project" ? (
              <ProjectScreen
                projectId={route.projectId}
                onBack={(teamId) => navigate(`/teams/${teamId}`)}
                onOpenCollections={(environmentId) =>
                  navigate(`/projects/${route.projectId}/environments/${environmentId}/collections`)
                }
                onOpenWorkspace={(environmentId) =>
                  navigate(
                    `/projects/${route.projectId}/environments/${environmentId}/${developerWorkspaceEnabled ? "overview" : "collections"}`,
                  )
                }
                onOpenFunctions={(environmentId) =>
                  navigate(`/projects/${route.projectId}/environments/${environmentId}/functions`)
                }
                onOpenObservability={(environmentId) =>
                  navigate(
                    `/projects/${route.projectId}/environments/${environmentId}/observability`,
                  )
                }
                onOpenUsers={(environmentId) =>
                  navigate(`/projects/${route.projectId}/environments/${environmentId}/users`)
                }
                onOpenSecurity={(environmentId) =>
                  navigate(`/projects/${route.projectId}/environments/${environmentId}/credentials`)
                }
              />
            ) : route.name === "environment_workspace" ? (
              developerWorkspaceEnabled ? (
                <EnvironmentWorkspaceScreen
                  projectId={route.projectId}
                  environmentId={route.environmentId}
                  section={route.section}
                  navigate={navigate}
                  explorerAdminEnabled={developerExplorerAdminEnabled}
                  dataJobsEnabled={developerDataJobsEnabled}
                  syncDetailsEnabled={developerSyncDetailsEnabled}
                  restoreEnabled={developerRestoreEnabled}
                />
              ) : (
                <LegacyWorkspaceFallback
                  projectId={route.projectId}
                  environmentId={route.environmentId}
                  navigate={navigate}
                />
              )
            ) : route.name === "collections" ? (
              <StagedEnvironmentLayout
                enabled={developerWorkspaceEnabled}
                projectId={route.projectId}
                environmentId={route.environmentId}
                section="collections"
                navigate={navigate}
              >
                <CollectionsScreen
                  projectId={route.projectId}
                  environmentId={route.environmentId}
                  onBack={() =>
                    navigate(
                      `/projects/${route.projectId}/environments/${route.environmentId}/overview`,
                    )
                  }
                  onOpen={(collectionId) =>
                    navigate(
                      `/projects/${route.projectId}/environments/${route.environmentId}/collections/${collectionId}`,
                    )
                  }
                />
              </StagedEnvironmentLayout>
            ) : route.name === "collection" ? (
              <StagedEnvironmentLayout
                enabled={developerWorkspaceEnabled}
                projectId={route.projectId}
                environmentId={route.environmentId}
                section="collections"
                navigate={navigate}
              >
                <CollectionScreen
                  projectId={route.projectId}
                  environmentId={route.environmentId}
                  collectionId={route.collectionId}
                  onOpenPolicies={() =>
                    navigate(
                      `/projects/${route.projectId}/environments/${route.environmentId}/collections/${route.collectionId}/policies`,
                    )
                  }
                  onBack={() =>
                    navigate(
                      `/projects/${route.projectId}/environments/${route.environmentId}/collections`,
                    )
                  }
                />
              </StagedEnvironmentLayout>
            ) : route.name === "policy" ? (
              <StagedEnvironmentLayout
                enabled={developerWorkspaceEnabled}
                projectId={route.projectId}
                environmentId={route.environmentId}
                section="policies"
                navigate={navigate}
              >
                <PolicyScreen
                  projectId={route.projectId}
                  environmentId={route.environmentId}
                  collectionId={route.collectionId}
                  onBack={() =>
                    navigate(
                      `/projects/${route.projectId}/environments/${route.environmentId}/collections/${route.collectionId}`,
                    )
                  }
                />
              </StagedEnvironmentLayout>
            ) : route.name === "users" ? (
              <StagedEnvironmentLayout
                enabled={developerWorkspaceEnabled}
                projectId={route.projectId}
                environmentId={route.environmentId}
                section="users"
                navigate={navigate}
              >
                <ApplicationUsersScreen
                  projectId={route.projectId}
                  environmentId={route.environmentId}
                  onBack={() =>
                    navigate(
                      `/projects/${route.projectId}/environments/${route.environmentId}/overview`,
                    )
                  }
                  onOpen={(userId) =>
                    navigate(
                      `/projects/${route.projectId}/environments/${route.environmentId}/users/${userId}`,
                    )
                  }
                />
              </StagedEnvironmentLayout>
            ) : route.name === "user" ? (
              <StagedEnvironmentLayout
                enabled={developerWorkspaceEnabled}
                projectId={route.projectId}
                environmentId={route.environmentId}
                section="users"
                navigate={navigate}
              >
                <ApplicationUserScreen
                  projectId={route.projectId}
                  environmentId={route.environmentId}
                  userId={route.userId}
                  onBack={() =>
                    navigate(
                      `/projects/${route.projectId}/environments/${route.environmentId}/users`,
                    )
                  }
                />
              </StagedEnvironmentLayout>
            ) : route.name === "credentials" ? (
              <StagedEnvironmentLayout
                enabled={developerWorkspaceEnabled}
                projectId={route.projectId}
                environmentId={route.environmentId}
                section="settings"
                navigate={navigate}
              >
                <CredentialsScreen
                  projectId={route.projectId}
                  environmentId={route.environmentId}
                  onBack={() =>
                    navigate(
                      `/projects/${route.projectId}/environments/${route.environmentId}/overview`,
                    )
                  }
                />
              </StagedEnvironmentLayout>
            ) : route.name === "functions" ? (
              <StagedEnvironmentLayout
                enabled={developerWorkspaceEnabled}
                projectId={route.projectId}
                environmentId={route.environmentId}
                section="functions"
                navigate={navigate}
              >
                <FunctionsScreen
                  projectId={route.projectId}
                  environmentId={route.environmentId}
                  onBack={() =>
                    navigate(
                      `/projects/${route.projectId}/environments/${route.environmentId}/overview`,
                    )
                  }
                  onOpen={(functionName) =>
                    navigate(
                      `/projects/${route.projectId}/environments/${route.environmentId}/functions/${functionName}`,
                    )
                  }
                />
              </StagedEnvironmentLayout>
            ) : route.name === "function" ? (
              <StagedEnvironmentLayout
                enabled={developerWorkspaceEnabled}
                projectId={route.projectId}
                environmentId={route.environmentId}
                section="functions"
                navigate={navigate}
              >
                <FunctionScreen
                  projectId={route.projectId}
                  environmentId={route.environmentId}
                  functionName={route.functionName}
                  onBack={() =>
                    navigate(
                      `/projects/${route.projectId}/environments/${route.environmentId}/functions`,
                    )
                  }
                />
              </StagedEnvironmentLayout>
            ) : route.name === "observability" ? (
              <StagedEnvironmentLayout
                enabled={developerWorkspaceEnabled}
                projectId={route.projectId}
                environmentId={route.environmentId}
                section="observability"
                navigate={navigate}
              >
                <ObservabilityScreen
                  projectId={route.projectId}
                  environmentId={route.environmentId}
                  onBack={() =>
                    navigate(
                      `/projects/${route.projectId}/environments/${route.environmentId}/overview`,
                    )
                  }
                />
              </StagedEnvironmentLayout>
            ) : (
              <NotFound path={route.path} onHome={() => navigate("/")} />
            )}
          </AuthenticatedShell>
        </RequireDeveloperSession>
      )}
    </ConsoleErrorBoundary>
  );
}

function LegacyWorkspaceFallback({
  projectId,
  environmentId,
  navigate,
}: {
  readonly projectId: string;
  readonly environmentId: string;
  readonly navigate: (path: string) => void;
}) {
  return (
    <section className="panel">
      <p className="eyebrow">Staged rollout</p>
      <h1>Developer workspace is disabled</h1>
      <p>
        The existing collection, user, function, credential, and observability pages remain
        available.
      </p>
      <div className="button-row">
        <button
          type="button"
          onClick={() =>
            navigate(`/projects/${projectId}/environments/${environmentId}/collections`)
          }
        >
          Open collections
        </button>
        <button
          type="button"
          className="secondary"
          onClick={() => navigate(`/projects/${projectId}`)}
        >
          Return to project
        </button>
      </div>
    </section>
  );
}

function StagedEnvironmentLayout({
  enabled,
  projectId,
  environmentId,
  section,
  navigate,
  children,
}: {
  readonly enabled: boolean;
  readonly projectId: string;
  readonly environmentId: string;
  readonly section: string;
  readonly navigate: (path: string, replace?: boolean) => void;
  readonly children: ReactNode;
}) {
  return enabled ? (
    <EnvironmentWorkspaceLayout
      projectId={projectId}
      environmentId={environmentId}
      section={section}
      navigate={navigate}
    >
      {children}
    </EnvironmentWorkspaceLayout>
  ) : (
    children
  );
}

function AuthenticatedShell({ children }: { readonly children: ReactNode }) {
  const { state, signOut } = useDeveloperAuth();
  if (state.status !== "authenticated") {
    return null;
  }
  if (state.session.audience !== "mako-management") {
    return null;
  }
  return (
    <div className="app-shell">
      <a className="skip-link" href="#main-content">
        Skip to main content
      </a>
      <header className="topbar">
        <div>
          <p className="eyebrow">Mako Cloud</p>
          <strong>Developer Console</strong>
        </div>
        <div className="account">
          <span>{state.session.profile.email}</span>
          <button type="button" className="secondary" onClick={() => void signOut()}>
            Sign out
          </button>
        </div>
      </header>
      <main className="workspace" id="main-content" tabIndex={-1}>
        {children}
      </main>
    </div>
  );
}

function NotFound({ path, onHome }: { readonly path: string; readonly onHome: () => void }) {
  return (
    <main className="centered">
      <section className="panel">
        <p className="eyebrow">404</p>
        <h1>Page not found</h1>
        <p>
          No console route matches <code>{path}</code>.
        </p>
        <button type="button" onClick={onHome}>
          Return home
        </button>
      </section>
    </main>
  );
}
