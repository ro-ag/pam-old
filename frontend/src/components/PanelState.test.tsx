import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { PanelEmpty, PanelError, PanelLoading } from "./PanelState";

describe("panel states", () => {
  it("announces a loading panel as a busy polite status", () => {
    render(<PanelLoading>Loading the connectors…</PanelLoading>);

    const state = screen.getByRole("status");
    expect(state).toHaveTextContent("Loading the connectors…");
    expect(state).toHaveAttribute("aria-busy", "true");
    expect(state).toHaveAttribute("aria-live", "polite");
    expect(state.tagName).toBe("P");
    expect(state).toHaveClass("panel-empty");
  });

  it("announces an error panel as an alert", () => {
    render(<PanelError>daemon socket unavailable</PanelError>);

    expect(screen.getByRole("alert")).toHaveTextContent("daemon socket unavailable");
  });

  it("leaves an empty panel unannounced", () => {
    render(<PanelEmpty>No callers are registered yet.</PanelEmpty>);

    expect(screen.queryByRole("status")).not.toBeInTheDocument();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    expect(screen.getByText("No callers are registered yet.")).toBeInTheDocument();
  });

  it("keeps the block skeleton — icon, heading, message, action — when a site needs it", () => {
    render(
      <PanelError
        as="section"
        className="panel loading-panel is-error"
        icon={<svg data-testid="warning" />}
        title="Flow workspace unavailable"
        action={<button type="button">Retry flows</button>}
      >
        the daemon said no
      </PanelError>,
    );

    const state = screen.getByRole("alert");
    expect(state.tagName).toBe("SECTION");
    expect(state).toHaveClass("panel", "loading-panel", "is-error");
    expect(screen.getByTestId("warning")).toBeInTheDocument();
    expect(state.querySelector("strong")).toHaveTextContent("Flow workspace unavailable");
    expect(state.querySelector("p")).toHaveTextContent("the daemon said no");
    expect(screen.getByRole("button", { name: "Retry flows" })).toBeInTheDocument();
  });
});
