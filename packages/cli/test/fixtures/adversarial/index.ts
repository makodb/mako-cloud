const projectSecret = Deno.env.get("PROJECT_SECRET") ?? "missing";
const projectMarker = Deno.env.get("PROJECT_MARKER") ?? "missing";

Deno.serve(async (request: Request) => {
  const url = new URL(request.url);
  if (url.pathname === "/inspect") {
    let processEnvironmentDenied = false;
    try {
      await Deno.readTextFile("/proc/1/environ");
    } catch {
      processEnvironmentDenied = true;
    }
    return Response.json({
      projectMarker,
      projectSecret,
      projectId: Deno.env.get("MAKO_PROJECT_ID"),
      environmentId: Deno.env.get("MAKO_ENVIRONMENT_ID"),
      undeclaredAbsent: Deno.env.get("OTHER_PROJECT_SECRET") === undefined,
      processEnvironmentDenied,
    });
  }
  if (url.pathname === "/leak") {
    console.error(`secret-bearing failure: ${projectSecret}`);
    throw new Error(`secret-bearing failure: ${projectSecret}`);
  }
  if (url.pathname === "/wall") {
    await new Promise((resolve) => setTimeout(resolve, 5_000));
    return new Response("wall limit failed");
  }
  if (url.pathname === "/memory") {
    const allocations: Uint8Array[] = [];
    for (;;) {
      const allocation = new Uint8Array(8 * 1024 * 1024);
      allocation.fill(0xa5);
      allocations.push(allocation);
    }
  }
  if (url.pathname === "/crash") {
    Deno.exit(86);
  }
  if (url.pathname === "/background") {
    const target = url.searchParams.get("target");
    if (target === null) return new Response("missing target", { status: 400 });
    setTimeout(() => {
      void fetch(target);
    }, 1_500);
    return new Response("scheduled");
  }
  if (url.pathname === "/safe") {
    return new Response(`safe:${projectMarker}`);
  }
  return new Response("not found", { status: 404 });
});
