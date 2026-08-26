import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import { createFixtureBridge, createTauriBridge } from "./bridge";
import type { ViewId } from "./domain";
import { fixtureScenario } from "./fixtures";
import { applyPamDensity, applyPamTheme, readPersistedPamDensity, readPersistedPamTheme, readPersistedPamThemeMode } from "./theme";
import "./styles.css";

const explicitFixtureMode = import.meta.env.DEV && import.meta.env.MODE === "fixture";
const query = explicitFixtureMode ? new URLSearchParams(window.location.search) : null;
const bridge = explicitFixtureMode ? createFixtureBridge(fixtureScenario(query?.get("scenario"))) : createTauriBridge();
const viewIds: readonly ViewId[] = ["control-center", "access", "skills", "flows", "activity", "console", "callers", "settings"];
const requestedView = query?.get("view");
const initialView: ViewId = viewIds.find((view) => view === requestedView) ?? "control-center";
const themeStorage = (() => {
  try { return window.localStorage; } catch { return null; }
})();
const initialTheme = readPersistedPamTheme(themeStorage);
const initialThemeMode = readPersistedPamThemeMode(themeStorage);
if ("__TAURI_INTERNALS__" in window && /Macintosh|Mac OS X/.test(window.navigator.userAgent)) {
  document.documentElement.dataset.nativeShell = "macos";
}
applyPamTheme(initialTheme, initialThemeMode);
applyPamDensity(readPersistedPamDensity(themeStorage));

const application = (
  <App bridge={bridge} initialView={initialView} initialTheme={initialTheme} initialThemeMode={initialThemeMode} />
);

createRoot(document.getElementById("root")!).render(
  explicitFixtureMode ? <StrictMode>{application}</StrictMode> : application,
);
