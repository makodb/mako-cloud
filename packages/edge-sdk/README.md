# `@mako-cloud/edge-sdk`

The edge SDK creates a client bound to the project, environment, and verified
caller supplied by the Mako runtime. Auth and document calls automatically send
that caller identity, so the same active document policies used for replication
also apply inside a function.

```ts
import { createFunctionClient } from "@mako-cloud/edge-sdk";

const mako = createFunctionClient(runtime.makoContext);
const user = await mako.auth.getUser();
const todos = mako.documents<{ ownerId: string; title: string }>("todos");
const page = await todos.query({
  predicates: [{ field: "ownerId", operator: "eq", value: user.id }],
  sort: [{ field: "ownerId", direction: "asc" }],
  cursor: null,
  limit: 100,
});
```

The default client has no privileged mode and accepts no per-operation token
override. If a public function invocation has no verified caller, auth and
document operations fail before making a network request.

Privileged access is a separate, explicit initialization using a service
credential from an attached secret. Every operation carries a required reason
and request identifier for the server-side bypass audit event:

```ts
import { createServiceClient } from "@mako-cloud/edge-sdk";

const service = createServiceClient({
  ...runtime.makoScope,
  serviceCredential: Deno.env.get("MAKO_MAINTENANCE_KEY")!,
  reason: "rebuild derived todo summaries",
  requestId: runtime.requestId,
});
```
