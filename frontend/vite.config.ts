import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import { configDefaults, defineConfig } from "vitest/config";
import packageJson from "./package.json";

export default defineConfig(({ mode }) => ({
  plugins: [react(), tailwindcss()],
  // The release gate keeps package.json, tauri.conf.json, and Cargo.toml
  // versions identical, so the bundled frontend version is the app version.
  //
  // Fixture mode pins it instead: the version label is rendered into every
  // full-canvas screenshot, so a real version here breaks every visual
  // baseline on each release bump — which is exactly how the baselines went
  // stale at v0.6.1. The fixture bridge already reports a pinned
  // `fixture-0.1.0` daemon version for the same reason.
  define: {
    __APP_VERSION__: JSON.stringify(mode === "fixture" ? "0.0.0-fixture" : (packageJson.version ?? "dev")),
  },
  clearScreen: false,
  server: {
    host: "127.0.0.1",
    port: 1420,
    strictPort: true,
  },
  build: {
    target: ["es2021", "chrome105", "safari13"],
    outDir: "dist",
    emptyOutDir: true,
    assetsInlineLimit: 0,
  },
  test: {
    environment: "jsdom",
    exclude: [...configDefaults.exclude, "e2e/**"],
    setupFiles: ["./vitest.setup.ts"],
    restoreMocks: true,
  },
}));
