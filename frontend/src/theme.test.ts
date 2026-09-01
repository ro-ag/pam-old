import { describe, expect, it, vi } from "vitest";
import { setTheme as setNativeTheme } from "@tauri-apps/api/app";
import {
  applyPamDensity,
  applyPamTheme,
  defaultPamDensity,
  pamDensityStorageKey,
  readPersistedPamDensity,
  storedPamDensity,
  writePersistedPamDensity,
  defaultPamTheme,
  defaultPamThemeMode,
  pamThemeModeStorageKey,
  pamThemeStorageKey,
  readPersistedPamTheme,
  readPersistedPamThemeMode,
  storedPamTheme,
  storedPamThemeMode,
  writePersistedPamTheme,
  writePersistedPamThemeMode,
} from "./theme";

vi.mock("@tauri-apps/api/app", () => ({
  setTheme: vi.fn().mockResolvedValue(undefined),
}));

describe("Pam themes", () => {
  it("accepts only the two named themes", () => {
    expect(storedPamTheme("ventisquero")).toBe("ventisquero");
    expect(storedPamTheme("vina")).toBe("vina");
    expect(storedPamTheme("night")).toBe(defaultPamTheme);
    expect(storedPamTheme(null)).toBe(defaultPamTheme);
  });

  it("accepts light and dark variants for either theme", () => {
    expect(storedPamThemeMode("light")).toBe("light");
    expect(storedPamThemeMode("dark")).toBe("dark");
    expect(storedPamThemeMode("system")).toBe(defaultPamThemeMode);
  });

  it("falls back when storage is unavailable", () => {
    expect(readPersistedPamTheme({
      getItem: () => { throw new Error("unavailable"); },
      setItem: vi.fn(),
    })).toBe(defaultPamTheme);
    expect(readPersistedPamThemeMode({
      getItem: () => { throw new Error("unavailable"); },
      setItem: vi.fn(),
    })).toBe(defaultPamThemeMode);
  });

  it("persists a selected theme under the Pam key", () => {
    const setItem = vi.fn();
    writePersistedPamTheme({ getItem: vi.fn(), setItem }, "vina");
    expect(setItem).toHaveBeenCalledWith(pamThemeStorageKey, "vina");
    writePersistedPamThemeMode({ getItem: vi.fn(), setItem }, "dark");
    expect(setItem).toHaveBeenCalledWith(pamThemeModeStorageKey, "dark");
  });

  it("defaults density to compact and persists it under the Pam key", () => {
    expect(storedPamDensity("comfortable")).toBe("comfortable");
    expect(storedPamDensity("compact")).toBe("compact");
    expect(storedPamDensity(null)).toBe(defaultPamDensity);
    expect(defaultPamDensity).toBe("compact");
    expect(readPersistedPamDensity({
      getItem: () => { throw new Error("unavailable"); },
      setItem: vi.fn(),
    })).toBe(defaultPamDensity);
    const setItem = vi.fn();
    writePersistedPamDensity({ getItem: vi.fn(), setItem }, "comfortable");
    expect(setItem).toHaveBeenCalledWith(pamDensityStorageKey, "comfortable");
  });

  it("applies density as a root data attribute keyed only on compact", () => {
    applyPamDensity("compact");
    expect(document.documentElement.dataset.density).toBe("compact");
    applyPamDensity("comfortable");
    expect(document.documentElement.dataset.density).toBeUndefined();
  });

  it("synchronizes native chrome only when the Tauri runtime is present", async () => {
    Object.defineProperty(window, "__TAURI_INTERNALS__", {
      configurable: true,
      value: {},
    });

    applyPamTheme("vina", "dark");
    await vi.waitFor(() => expect(setNativeTheme).toHaveBeenCalledWith("dark"));
    applyPamTheme("ventisquero", "dark");
    expect(setNativeTheme).toHaveBeenCalledTimes(1);

    Reflect.deleteProperty(window, "__TAURI_INTERNALS__");
  });
});
