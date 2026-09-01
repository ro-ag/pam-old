import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { fixtureBridge, type FixtureScenario } from "../fixtures";
import { ConnectorsPanel } from "./ConnectorsPanel";

function connectorsProps(scenario: FixtureScenario = "solved") {
  return { bridge: fixtureBridge(scenario) };
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((onResolve) => { resolve = onResolve; });
  return { promise, resolve };
}

describe("ConnectorsPanel", () => {
  it("renders a configured connector with its credential and last-test pills", async () => {
    const props = connectorsProps();
    render(<ConnectorsPanel {...props} />);

    const row = (await screen.findByText("github-actions")).closest("article")!;
    expect(within(row).getByText("credential stored")).toBeInTheDocument();
    expect(within(row).getByText(/^test passed · .+ago$/)).toBeInTheDocument();
    expect(within(row).getByRole("switch")).toBeChecked();
    expect(within(row).getByLabelText("Base URL")).toHaveValue("https://api.github.com");
  });

  it("renders the dormant unconfigured connector state", async () => {
    const props = connectorsProps("connector-unconfigured");
    render(<ConnectorsPanel {...props} />);

    const row = (await screen.findByText("github-actions")).closest("article")!;
    expect(within(row).getByText("no credential")).toBeInTheDocument();
    expect(within(row).getByText("never tested")).toBeInTheDocument();
    expect(within(row).getByRole("switch")).not.toBeChecked();
    expect(within(row).getByLabelText("Base URL")).toHaveValue("");
    expect(within(row).getByRole("button", { name: "Add credential…" })).toBeInTheDocument();
    expect(within(row).queryByRole("button", { name: "Remove credential" })).not.toBeInTheDocument();
  });

  it("shows the calm empty and offline registry states", async () => {
    const empty = connectorsProps("empty");
    const { unmount } = render(<ConnectorsPanel {...empty} />);
    expect(await screen.findByText("No connectors are registered with the daemon yet.")).toBeInTheDocument();
    unmount();

    const offline = connectorsProps("offline");
    render(<ConnectorsPanel {...offline} />);
    expect(await screen.findByText(/connector registry is not being served/)).toBeInTheDocument();
    expect(screen.getByText(/Start Pam to read the connectors/)).toBeInTheDocument();
  });

  it("saves the enabled switch and base URL through configure with exact params", async () => {
    const user = userEvent.setup();
    const props = connectorsProps("connector-unconfigured");
    const spy = vi.spyOn(props.bridge, "connectorConfigure");
    render(<ConnectorsPanel {...props} />);

    const row = (await screen.findByText("github-actions")).closest("article")!;
    await user.click(within(row).getByRole("switch"));
    await user.type(within(row).getByLabelText("Base URL"), "https://api.github.com");
    await user.click(within(row).getByRole("button", { name: "Save" }));

    await waitFor(() => expect(spy).toHaveBeenCalledTimes(1));
    expect(spy).toHaveBeenCalledWith(expect.objectContaining({ projectHandle: "daemon", generation: "daemon" }), {
      connector: "github-actions",
      enabled: true,
      baseUrl: "https://api.github.com",
    });
    await waitFor(() => expect(within(row).getByRole("switch")).toBeChecked());
  });

  it("sets a credential without ever echoing the secret after submit", async () => {
    const user = userEvent.setup();
    const props = connectorsProps("connector-unconfigured");
    const spy = vi.spyOn(props.bridge, "connectorConfigure");
    render(<ConnectorsPanel {...props} />);

    const row = (await screen.findByText("github-actions")).closest("article")!;
    await user.click(within(row).getByRole("button", { name: "Add credential…" }));
    const input = within(row).getByLabelText("Credential");
    expect(input).toHaveAttribute("type", "password");
    await user.type(input, "ghp-fixture-secret-token");
    await user.click(within(row).getByRole("button", { name: "Save credential" }));

    await waitFor(() => expect(spy).toHaveBeenCalledWith(expect.anything(), {
      connector: "github-actions",
      credential: { action: "set", secret: "ghp-fixture-secret-token" },
    }));
    await waitFor(() => expect(within(row).getByText("credential stored")).toBeInTheDocument());
    expect(within(row).queryByLabelText("Credential")).not.toBeInTheDocument();
    expect(screen.queryByDisplayValue("ghp-fixture-secret-token")).not.toBeInTheDocument();
    expect(document.body.innerHTML).not.toContain("ghp-fixture-secret-token");
  });

  it("clears a stored credential only after an inline confirmation", async () => {
    const user = userEvent.setup();
    const props = connectorsProps();
    const spy = vi.spyOn(props.bridge, "connectorConfigure");
    render(<ConnectorsPanel {...props} />);

    const row = (await screen.findByText("github-actions")).closest("article")!;
    await user.click(within(row).getByRole("button", { name: "Remove credential" }));
    expect(spy).not.toHaveBeenCalled();
    expect(within(row).getByText("Remove the stored credential?")).toBeInTheDocument();

    await user.click(within(row).getByRole("button", { name: "Keep" }));
    expect(spy).not.toHaveBeenCalled();

    await user.click(within(row).getByRole("button", { name: "Remove credential" }));
    await user.click(within(row).getByRole("button", { name: "Remove" }));
    await waitFor(() => expect(spy).toHaveBeenCalledWith(expect.anything(), {
      connector: "github-actions",
      credential: { action: "clear" },
    }));
    await waitFor(() => expect(within(row).getByText("no credential")).toBeInTheDocument());
  });

  it("tests the connection with a busy spinner and updates the row with the result", async () => {
    const user = userEvent.setup();
    const props = connectorsProps();
    const pending = deferred<Awaited<ReturnType<typeof props.bridge.connectorTest>>>();
    const spy = vi.spyOn(props.bridge, "connectorTest").mockReturnValue(pending.promise);
    render(<ConnectorsPanel {...props} />);

    const row = (await screen.findByText("github-actions")).closest("article")!;
    const testButton = within(row).getByRole("button", { name: "Test connection" });
    await user.click(testButton);
    expect(spy).toHaveBeenCalledWith(expect.objectContaining({ projectHandle: "daemon", generation: "daemon" }), "github-actions");
    expect(testButton).toBeDisabled();
    expect(within(row).getByRole("button", { name: "Save" })).toBeDisabled();
    expect(testButton.querySelector(".is-spinning")).not.toBeNull();

    pending.resolve({ status: "ok", connectorId: "github-actions", result: "passed", detail: "The connector answered the bounded test call." });
    await waitFor(() => expect(testButton).toBeEnabled());
    expect(within(row).getByText(/^test passed/)).toBeInTheDocument();
    expect(within(row).getByText("The connector answered the bounded test call.")).toBeInTheDocument();
  });

  it("renders a failed test result with its detail", async () => {
    const user = userEvent.setup();
    const props = connectorsProps("connector-unconfigured");
    render(<ConnectorsPanel {...props} />);

    const row = (await screen.findByText("github-actions")).closest("article")!;
    await user.click(within(row).getByRole("button", { name: "Test connection" }));
    expect(await within(row).findByText(/^test failed/)).toBeInTheDocument();
    expect(within(row).getByText(/needs a base URL and a stored credential/)).toBeInTheDocument();
  });

  it("renders blocked-grant recovery calmly for configure and test", async () => {
    const user = userEvent.setup();
    const props = connectorsProps("connector-blocked");
    render(<ConnectorsPanel {...props} />);

    const row = (await screen.findByText("github-actions")).closest("article")!;
    await user.click(within(row).getByRole("button", { name: "Save" }));
    expect(await within(row).findByRole("alert")).toHaveTextContent(
      "pam access grant connector.configure for this GUI caller and project, then retry.",
    );

    await user.click(within(row).getByRole("button", { name: "Test connection" }));
    await waitFor(() => expect(within(row).getByRole("alert")).toHaveTextContent(
      "pam access grant connector.test for this GUI caller and project, then retry.",
    ));
    expect(within(row).getByText("never tested")).toBeInTheDocument();
  });
});
