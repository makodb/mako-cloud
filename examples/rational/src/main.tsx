import { createRoot } from "react-dom/client";

import { resolveConfig } from "./config.js";
import { RationalApp } from "./data/rational.js";
import { FakeMakoBackend } from "./testing/fake-backend.js";
import { App } from "./ui/app.jsx";
import "./styles.css";

/**
 * The app object is created once and exposed as `window.rational` so the
 * browser suites drive the same object the screens render.
 */
declare global {
  interface Window {
    rational: RationalApp;
    rationalFake?: FakeMakoBackend;
  }
}

const config = resolveConfig(window.__RATIONAL__);
const fake = config.mode === "fake" ? new FakeMakoBackend() : null;
const app = new RationalApp({
  config,
  ...(fake === null ? {} : { fetch: fake.fetch, now: fake.now }),
});
window.rational = app;
if (fake !== null) window.rationalFake = fake;

const rootElement = document.querySelector("#root");
if (rootElement === null) throw new Error("Rational root element is missing");
createRoot(rootElement).render(<App app={app} />);
void app.start();
window.addEventListener("pagehide", () => void app.close());
