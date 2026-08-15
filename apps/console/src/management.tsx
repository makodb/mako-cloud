import { createContext, type ReactNode, useContext, useMemo } from "react";

import { createManagementClient, type MakoManagementClient } from "@mako-cloud/management-sdk";

import { isSessionActive, useDeveloperAuth } from "./auth.js";

const ManagementContext = createContext<MakoManagementClient | null>(null);

export function ManagementProvider({
  endpoint,
  children,
}: {
  readonly endpoint: string;
  readonly children: ReactNode;
}) {
  const { state } = useDeveloperAuth();
  const client = useMemo(() => {
    if (state.status !== "authenticated" || state.session.audience !== "mako-management") {
      return null;
    }
    const session = state.session;
    return createManagementClient({
      endpoint,
      credential: {
        kind: "developer_session",
        accessToken: () => {
          if (!isSessionActive(session)) {
            throw new DeveloperSessionExpiredError();
          }
          return session.accessToken;
        },
      },
    });
  }, [endpoint, state]);

  return <ManagementContext.Provider value={client}>{children}</ManagementContext.Provider>;
}

export function useManagementClient(): MakoManagementClient {
  const client = useContext(ManagementContext);
  if (client === null) {
    throw new DeveloperSessionExpiredError();
  }
  return client;
}

export class DeveloperSessionExpiredError extends Error {
  override readonly name = "DeveloperSessionExpiredError";

  constructor() {
    super("Your developer session expired. Sign in again to continue.");
  }
}
