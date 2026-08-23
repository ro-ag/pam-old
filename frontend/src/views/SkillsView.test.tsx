import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { CommandFence, ProjectSummaryDto } from "../domain";
import { fixtureBridge } from "../fixtures";
import { SkillsView } from "./SkillsView";

const projects: ProjectSummaryDto[] = [
  { handle: "11111111-1111-4111-8111-111111111111", name: "payments-api", location: "/work/payments-api" },
];

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
    render(<SkillsView bridge={bridge} fence={null} projects={projects} onSelectProject={vi.fn()} />);

    expect(await screen.findByText("Global review checklist")).toBeInTheDocument();

    await user.click(screen.getByRole("tab", { name: "Audit" }));
    expect(await screen.findByRole("heading", { name: "Evaluator verdict" })).toBeInTheDocument();

    const fences = [...inventoryFences, ...auditFences];
    expect(fences.length).toBeGreaterThanOrEqual(2);
    expect(fences.every((fence) => fence.projectHandle === "daemon" && fence.generation === "daemon")).toBe(true);
    expect(new Set(fences.map((fence) => fence.operationId)).size).toBe(fences.length);
  });

  it("gates library assignment on a project picked from the panel itself", async () => {
    const user = userEvent.setup();
    const onSelectProject = vi.fn();
    render(<SkillsView bridge={fixtureBridge()} fence={null} projects={projects} onSelectProject={onSelectProject} />);

    await user.click(screen.getByRole("tab", { name: "Library" }));
    expect(await screen.findByText("review-changes")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Adopt into library" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Enable target" })).not.toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: /payments-api/ }));
    expect(onSelectProject).toHaveBeenCalledWith(projects[0]);
  });
});
