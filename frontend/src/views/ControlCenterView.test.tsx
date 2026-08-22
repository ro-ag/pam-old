import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { fixtureBridge } from "../fixtures";
import { selectControlCenter } from "../selectors";
import { ControlCenterView } from "./ControlCenterView";

async function controlCenterProps() {
  const bridge = fixtureBridge();
  const snapshot = await bridge.bootstrap();
  const catalog = await bridge.catalog();
  return {
    data: selectControlCenter(snapshot.data, catalog, true),
    onCopy: vi.fn(),
    onEvidence: vi.fn(),
    onContinue: vi.fn(),
    onOpenQueue: vi.fn(),
    onOpenApproval: vi.fn(),
    onRecoverDaemon: vi.fn(),
    onRefresh: vi.fn(),
    onRegisterCaller: vi.fn(),
    registrationBusy: false,
  };
}

describe("ControlCenterView", () => {
  it("hosts the project control center content in a standalone canvas", async () => {
    const props = await controlCenterProps();
    render(<ControlCenterView {...props} />);

    expect(screen.getByRole("heading", { name: "Control center" })).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "payments-api" })).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "Ready for the next agent" })).toBeInTheDocument();
    expect(screen.getByText("Watch status")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Open Evidence 1" })).toBeInTheDocument();
  });
});
