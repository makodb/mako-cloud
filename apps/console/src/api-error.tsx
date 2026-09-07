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
  return { message: "The management request failed.", requestId: null };
}

export function ApiFailureNotice({ failure }: { readonly failure: ConsoleApiFailure | null }) {
  if (failure === null) {
    return null;
  }
  return (
    <Alert variant="destructive" role="alert">
      <CircleAlert aria-hidden="true" />
      <AlertTitle>{failure.message}</AlertTitle>
      {failure.requestId === null ? null : (
        <AlertDescription>
          <RequestId value={failure.requestId} />
        </AlertDescription>
      )}
    </Alert>
  );
}
