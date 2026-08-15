import { nanoid } from "npm:nanoid@5.1.5";

import { javascriptModuleValue } from "./helper.js";

const encoder = new TextEncoder();

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
        Deno.env.get("UNDECLARED_VALUE") === undefined && Deno.env.get("MAKO_JWKS") === undefined,
      functionPath: url.pathname,
      query: url.search,
    });
  }
  if (url.pathname === "/outbound") {
    const target = url.searchParams.get("target");
    if (target === null) return new Response("missing target", { status: 400 });
    const response = await fetch(target);
    return new Response(await response.text(), {
      status: response.status,
      headers: { "x-upstream-result": response.headers.get("x-test-upstream") ?? "missing" },
    });
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
