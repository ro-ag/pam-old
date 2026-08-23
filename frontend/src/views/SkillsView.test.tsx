import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
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

    await user.click(screen.getByRole("tab", { name: "Audit" }));
    expect(await screen.findByRole("heading", { name: "Skill audit" })).toBeInTheDocument();
  });
});
