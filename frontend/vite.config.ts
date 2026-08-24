import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import { configDefaults, defineConfig } from "vitest/config";
import packageJson from "./package.json";

export default defineConfig({
  plugins: [react(), tailwindcss()],
  // The release gate keeps package.json, tauri.conf.json, and Cargo.toml
  // versions identical, so the bundled frontend version is the app version.
  define: {
    __APP_VERSION__: JSON.stringify(packageJson.version ?? "dev"),
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
});
