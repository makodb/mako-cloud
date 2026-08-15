import { ManagementApiError } from "@mako-cloud/management-sdk";

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
    <div className="notice error" role="alert">
      <p>{failure.message}</p>
      <RequestId value={failure.requestId} />
    </div>
  );
}
