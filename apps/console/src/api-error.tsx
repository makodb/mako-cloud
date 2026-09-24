import { ManagementApiError } from "@mako-cloud/management-sdk";
import { Alert, AlertDescription, AlertTitle } from "@mako-cloud/ui";
import { CircleAlert } from "lucide-react";

import { RequestId } from "./error-boundary.js";

export interface ConsoleApiFailure {
  readonly message: string;
  readonly requestId: string | null;
}

export function toConsoleApiFailure(error: unknown): ConsoleApiFailure {
  if (error instanceof ManagementApiError) {
    return { message: error.message, requestId: error.requestId };
  }
  if (isNetworkFailure(error)) {
    return {
      message: "The management API could not be reached. Check the connection, then try again.",
      requestId: null,
    };
  }
  return { message: "The management request failed.", requestId: null };
}

/**
 * Whether no answer arrived at all: `fetch` rejects with a TypeError when the
 * network drops, a proxy or tunnel in front of the service is down, or the
 * service is restarting. Retrying is the remedy there, so it is worth telling
 * apart from a refusal. Each engine words it differently.
 */
function isNetworkFailure(error: unknown): boolean {
  return (
    error instanceof TypeError &&
    /failed to fetch|networkerror|load failed|network connection was lost/iu.test(error.message)
  );
}

export function ApiFailureNotice({ failure }: { readonly failure: ConsoleApiFailure | null }) {
  if (failure === null) {
    return null;
  }
  return (
    <Alert variant="destructive" role="alert">
      <CircleAlert aria-hidden="true" />
      <AlertTitle>The management request failed</AlertTitle>
      <AlertDescription>
        <p className="m-0 text-foreground">{failure.message}</p>
        <RequestId value={failure.requestId} />
      </AlertDescription>
    </Alert>
  );
}
