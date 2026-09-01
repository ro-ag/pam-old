import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { fixtureBridge } from "../fixtures";
import { DaemonAccessPanel } from "./DaemonAccessPanel";

const row = async (capability: string) => within(await screen.findByRole("article", { name: capability }));

describe("DaemonAccessPanel", () => {
  it("reads the daemon-scope grant state under the daemon authority", async () => {
    const bridge = fixtureBridge("connector-blocked");
    const read = vi.spyOn(bridge, "daemonAccess");
    render(<DaemonAccessPanel bridge={bridge} />);

    expect(within(await screen.findByRole("article", { name: "model.infer" })).getByText("granted")).toBeInTheDocument();
    expect((await row("connector.configure")).getByText("not granted")).toBeInTheDocument();
    expect((await row("connector.test")).getByRole("button", { name: "Grant" })).toBeInTheDocument();
    expect(read.mock.calls[0][0]).toMatchObject({ projectHandle: "daemon", generation: "daemon" });
  });

  it("grants a denied capability and reflects the result", async () => {
    const user = userEvent.setup();
    const bridge = fixtureBridge("connector-blocked");
    const write = vi.spyOn(bridge, "setDaemonAccess");
    render(<DaemonAccessPanel bridge={bridge} />);

    await user.click((await row("connector.test")).getByRole("button", { name: "Grant" }));

    await waitFor(async () => expect((await row("connector.test")).getByText("granted")).toBeInTheDocument());
    expect((await row("connector.test")).getByRole("button", { name: "Revoke" })).toBeInTheDocument();
    expect(write).toHaveBeenCalledWith(
      expect.objectContaining({ projectHandle: "daemon", generation: "daemon" }),
      "connector.test",
      true,
    );
  });

  it("revokes a granted capability", async () => {
    const user = userEvent.setup();
    const bridge = fixtureBridge();
    render(<DaemonAccessPanel bridge={bridge} />);

    await user.click((await row("model.infer")).getByRole("button", { name: "Revoke" }));

    await waitFor(async () => expect((await row("model.infer")).getByText("not granted")).toBeInTheDocument());
  });

  it("reports a refused grant beside its row and keeps the state honest", async () => {
    const user = userEvent.setup();
    const bridge = fixtureBridge("connector-blocked");
    bridge.setDaemonAccess = vi.fn(async () => {
      throw new Error("Pam could not record this capability grant.");
    });
    render(<DaemonAccessPanel bridge={bridge} />);

    await user.click((await row("connector.test")).getByRole("button", { name: "Grant" }));

    expect((await row("connector.test")).getByRole("alert")).toHaveTextContent("Pam could not record this capability grant.");
    expect((await row("connector.test")).getByText("not granted")).toBeInTheDocument();
  });

  it("tells its host to re-read after a grant and after a revoke", async () => {
    const user = userEvent.setup();
    const bridge = fixtureBridge("connector-blocked");
    const onGrantsChanged = vi.fn();
    render(<DaemonAccessPanel bridge={bridge} onGrantsChanged={onGrantsChanged} />);

    await user.click((await row("connector.test")).getByRole("button", { name: "Grant" }));
    await waitFor(() => expect(onGrantsChanged).toHaveBeenCalledTimes(1));

    await user.click((await row("model.infer")).getByRole("button", { name: "Revoke" }));
    await waitFor(() => expect(onGrantsChanged).toHaveBeenCalledTimes(2));
  });
});
