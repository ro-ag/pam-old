import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { fixtureBridge, type FixtureScenario } from "../fixtures";
import { ConnectionsView } from "./ConnectionsView";

async function connectionsProps(scenario: FixtureScenario = "solved") {
  const bridge = fixtureBridge(scenario);
  const snapshot = await bridge.bootstrap();
  return { bridge, fence: snapshot.fence };
}

describe("ConnectionsView", () => {
  it("hosts the Callers and Connectors panels behind tabs, callers first", async () => {
    const props = await connectionsProps();
    render(<ConnectionsView {...props} />);

    expect(screen.getByRole("heading", { name: "Connections" })).toBeInTheDocument();
    const tabs = screen.getAllByRole("tab");
    expect(tabs.map((tab) => tab.textContent)).toEqual(["Callers", "Connectors"]);
    expect(await screen.findByRole("heading", { name: "Registered callers" })).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "Connectors" })).not.toBeInTheDocument();
  });

  it("switches to the Connectors panel on demand", async () => {
    const user = userEvent.setup();
    const props = await connectionsProps();
    render(<ConnectionsView {...props} />);

    await user.click(screen.getByRole("tab", { name: "Connectors" }));
    expect(await screen.findByRole("heading", { name: "Connectors" })).toBeInTheDocument();
    expect(screen.getByText("Connectors stay off until you enable them, add a credential, and run a test.")).toBeInTheDocument();
    expect(await screen.findByText("github-actions")).toBeInTheDocument();
  });

  it("lists registered callers with registration dates and revoked badges", async () => {
    const props = await connectionsProps();
    render(<ConnectionsView {...props} />);

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
    const props = await connectionsProps();
    const spy = vi.spyOn(props.bridge, "callerRegistry");
    render(<ConnectionsView {...props} />);
    await screen.findByText("gui:pam-desktop");
    expect(spy).toHaveBeenCalledTimes(1);

    await user.click(screen.getByRole("button", { name: "Refresh callers" }));
    await waitFor(() => expect(spy).toHaveBeenCalledTimes(2));
  });

  it("explains the paused registry while the daemon is offline", async () => {
    const props = await connectionsProps("offline");
    render(<ConnectionsView {...props} />);

    expect(await screen.findByText(/caller registry is not being served/)).toBeInTheDocument();
    expect(screen.getByText(/Start PAM to read the registered callers/)).toBeInTheDocument();
  });
});
