import { nanoid } from "npm:nanoid@5.1.5";

import { javascriptModuleValue } from "./helper.js";

const encoder = new TextEncoder();

/**
 * A worker holds the names the platform attached to it and no others. Reading
 * one that was not attached is refused rather than answered with `undefined`,
 * and the refusal throws.
 */
function environmentIsUnreadable(name: string): boolean {
  try {
    return Deno.env.get(name) === undefined;
  } catch {
    return true;
  }
}

Deno.serve(async (request: Request) => {
  const url = new URL(request.url);
  if (url.pathname === "/features") {
    const addModule = await WebAssembly.instantiate(
      new Uint8Array([
        0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, 0x01, 0x07, 0x01, 0x60, 0x02, 0x7f, 0x7f,
        0x01, 0x7f, 0x03, 0x02, 0x01, 0x00, 0x07, 0x07, 0x01, 0x03, 0x61, 0x64, 0x64, 0x00, 0x00,
        0x0a, 0x09, 0x01, 0x07, 0x00, 0x20, 0x00, 0x20, 0x01, 0x6a, 0x0b,
      ]),
    );
    const add = addModule.instance.exports.add as (left: number, right: number) => number;
    const typedValue: string = "typescript-ok";
    return Response.json({
      fetchApi: request instanceof Request && new Headers() instanceof Headers,
      typeScript: typedValue,
      javaScript: javascriptModuleValue(),
      npm: nanoid(12).length,
      webAssembly: add(20, 22),
      environment: Deno.env.get("FUNCTION_MODE"),
      secret: Deno.env.get("TEST_SECRET"),
      undeclaredEnvironmentAbsent:
        environmentIsUnreadable("UNDECLARED_VALUE") && environmentIsUnreadable("MAKO_JWKS"),
      functionPath: url.pathname,
      query: url.search,
    });
  }
  if (url.pathname === "/outbound") {
    // A function's own destinations are denied: protocol v1 grants a worker
    // the platform API origin and nothing else, so this reports whether the
    // attempt was refused rather than what it reached.
    const target = url.searchParams.get("target");
    if (target === null) return new Response("missing target", { status: 400 });
    try {
      const response = await fetch(target);
      return new Response(await response.text(), {
        status: response.status,
        headers: { "x-upstream-result": response.headers.get("x-test-upstream") ?? "missing" },
      });
    } catch (error) {
      const thrown = error as { name?: string; constructor?: { name?: string } };
      return new Response("outbound-denied", {
        headers: {
          "x-upstream-result": "denied",
          "x-upstream-error": thrown?.name ?? thrown?.constructor?.name ?? "unknown",
        },
      });
    }
  }
  if (url.pathname === "/stream") {
    return new Response(
      new ReadableStream({
        async start(controller) {
          controller.enqueue(encoder.encode("stream-"));
          await new Promise((resolve) => setTimeout(resolve, 20));
          controller.enqueue(encoder.encode("response"));
          controller.close();
        },
      }),
      { headers: { "content-type": "text/plain" } },
    );
  }
  if (url.pathname === "/echo") {
    return Response.json({ method: request.method, body: await request.text() });
  }
  return new Response("not found", { status: 404 });
});
