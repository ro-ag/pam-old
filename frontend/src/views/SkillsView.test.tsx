import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { CommandFence } from "../domain";
import { fixtureBridge } from "../fixtures";
import { SkillsView } from "./SkillsView";

describe("SkillsView", () => {
  it("hosts the three skill panels as Inventory, Library, and Audit tabs", async () => {
    const user = userEvent.setup();
    const bridge = fixtureBridge();
    const snapshot = (await bridge.bootstrap()).snapshot!;
    render(<SkillsView bridge={bridge} fence={snapshot.fence} />);

    expect(screen.getByRole("heading", { name: "Skills" })).toBeInTheDocument();
    const tabs = screen.getAllByRole("tab").map((tab) => tab.textContent);
    expect(tabs).toEqual(["Inventory", "Library", "Audit"]);
    expect(screen.getByRole("tab", { name: "Inventory" })).toHaveAttribute("aria-selected", "true");
    expect(await screen.findByRole("heading", { name: "Skill inventory" })).toBeInTheDocument();

    await user.click(screen.getByRole("tab", { name: "Library" }));
    expect(await screen.findByRole("heading", { name: "Skill library" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Enable target" })).toBeInTheDocument();

    await user.click(screen.getByRole("tab", { name: "Audit" }));
    expect(await screen.findByRole("heading", { name: "Skill audit" })).toBeInTheDocument();
  });

  it("renders global skills on the daemon authority when no project is active", async () => {
    const user = userEvent.setup();
    const bridge = fixtureBridge();
    const inventoryFences: CommandFence[] = [];
    const auditFences: CommandFence[] = [];
    const nativeInventory = bridge.loadSkillInventory.bind(bridge);
    const nativeAudit = bridge.loadSkillAudit.bind(bridge);
    bridge.loadSkillInventory = vi.fn(async (fence) => { inventoryFences.push({ ...fence }); return nativeInventory(fence); });
    bridge.loadSkillAudit = vi.fn(async (fence) => { auditFences.push({ ...fence }); return nativeAudit(fence); });
    render(<SkillsView bridge={bridge} fence={null} />);

    expect(await screen.findByText("Global review checklist")).toBeInTheDocument();

    await user.click(screen.getByRole("tab", { name: "Audit" }));
    expect(await screen.findByRole("heading", { name: "Evaluator verdict" })).toBeInTheDocument();

    const fences = [...inventoryFences, ...auditFences];
    expect(fences.length).toBeGreaterThanOrEqual(2);
    expect(fences.every((fence) => fence.projectHandle === "daemon" && fence.generation === "daemon")).toBe(true);
    expect(new Set(fences.map((fence) => fence.operationId)).size).toBe(fences.length);
  });

  it("carries no project identity: no switcher, no picker, library still readable", async () => {
    const user = userEvent.setup();
    render(<SkillsView bridge={fixtureBridge()} fence={null} />);

    expect(screen.queryByRole("button", { name: /payments-api/ })).not.toBeInTheDocument();

    await user.click(screen.getByRole("tab", { name: "Library" }));
    expect(await screen.findByText("review-changes")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Adopt into library" })).toBeInTheDocument();
    // Assignment stays gated without a project scope, but nothing offers a pick.
    expect(screen.queryByRole("button", { name: "Enable target" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /payments-api/ })).not.toBeInTheDocument();
    expect(screen.getByText(/PAM has none open/)).toBeInTheDocument();
  });
});
