import { StrictMode } from "react";
import { createRoot, type Root } from "react-dom/client";

import { ConsoleApp } from "./app.js";
import { DeveloperAuthProvider, type DeveloperAuthAdapter } from "./auth.js";
import { ManagementProvider } from "./management.js";
import { OperatorAuthProvider, type OperatorAuthAdapter } from "./operator-auth.js";
import { OperatorManagementProvider } from "./operator-management.js";

export interface ConsoleBootstrapOptions {
  readonly managementEndpoint: string;
  readonly developerAuth: DeveloperAuthAdapter;
  readonly operatorAuth?: OperatorAuthAdapter;
  /** Gates the new nested developer workspace while legacy project routes remain available. */
  readonly developerWorkspaceEnabled?: boolean;
  /** Enables policy-bypassing mutations independently of read-only preview. */
  readonly developerExplorerAdminEnabled?: boolean;
  /** Enables import and export jobs independently of interactive exploration. */
  readonly developerDataJobsEnabled?: boolean;
  /** Enables detailed RxDB aggregate diagnostics. */
  readonly developerSyncDetailsEnabled?: boolean;
  /** Enables stepped-up isolated recovery requests. */
  readonly developerRestoreEnabled?: boolean;
}

export function mountConsole(element: Element, options: ConsoleBootstrapOptions): Root {
  const root = createRoot(element);
  root.render(
    <StrictMode>
      <OperatorAuthProvider adapter={options.operatorAuth}>
        <OperatorManagementProvider endpoint={options.managementEndpoint}>
          <DeveloperAuthProvider adapter={options.developerAuth}>
            <ManagementProvider endpoint={options.managementEndpoint}>
              <ConsoleApp
                developerWorkspaceEnabled={options.developerWorkspaceEnabled !== false}
                developerExplorerAdminEnabled={options.developerExplorerAdminEnabled !== false}
                developerDataJobsEnabled={options.developerDataJobsEnabled !== false}
                developerSyncDetailsEnabled={options.developerSyncDetailsEnabled !== false}
                developerRestoreEnabled={options.developerRestoreEnabled !== false}
              />
            </ManagementProvider>
          </DeveloperAuthProvider>
        </OperatorManagementProvider>
      </OperatorAuthProvider>
    </StrictMode>,
  );
  return root;
}
