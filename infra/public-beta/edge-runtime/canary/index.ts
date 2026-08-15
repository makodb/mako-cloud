Deno.serve(() =>
  Response.json({
    status: "ok",
    runtimeProtocol: 1,
  }),
);
