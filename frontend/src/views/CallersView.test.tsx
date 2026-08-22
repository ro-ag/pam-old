import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { fixtureBridge, type FixtureScenario } from "../fixtures";
import { CallersView } from "./CallersView";

async function callersProps(scenario: FixtureScenario = "solved") {
  const bridge = fixtureBridge(scenario);
  const snapshot = await bridge.bootstrap();
  return { bridge, fence: snapshot.fence };
}

describe("CallersView", () => {
  it("lists registered callers with registration dates and revoked badges", async () => {
    const props = await callersProps();
    render(<CallersView {...props} />);

    expect(screen.getByRole("heading", { name: "Callers" })).toBeInTheDocument();
    expect(await screen.findByText("gui:pam-desktop")).toBeInTheDocument();
    expect(screen.getByText("cli:release-agent")).toBeInTheDocument();
    const revokedRow = screen.getByText("cli:retired-agent").closest("article");
    expect(revokedRow).not.toBeNull();
    expect(within(revokedRow!).getByText("revoked")).toBeInTheDocument();
    expect(screen.getAllByText("active")).toHaveLength(2);
    expect(screen.getAllByText(/^Registered .*\d{4}$/)).toHaveLength(3);
  });

  it("refreshes the caller registry on demand", async () => {
    const user = userEvent.setup();
    const props = await callersProps();
    const spy = vi.spyOn(props.bridge, "callerRegistry");
    render(<CallersView {...props} />);
    await screen.findByText("gui:pam-desktop");
    expect(spy).toHaveBeenCalledTimes(1);

    await user.click(screen.getByRole("button", { name: "Refresh callers" }));
    await waitFor(() => expect(spy).toHaveBeenCalledTimes(2));
  });

  it("explains the paused registry while the daemon is offline", async () => {
    const props = await callersProps("offline");
    render(<CallersView {...props} />);

    expect(await screen.findByText(/caller registry is not being served/)).toBeInTheDocument();
    expect(screen.getByText(/Start PAM to read the registered callers/)).toBeInTheDocument();
  });
});
