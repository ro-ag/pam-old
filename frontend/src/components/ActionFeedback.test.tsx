import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { ConfirmAction, FailureNotice, SuccessNotice } from "./ActionFeedback";

describe("ConfirmAction", () => {
  it("asks before it fires, and cancels without firing", async () => {
    const onConfirm = vi.fn();
    const onCancel = vi.fn();
    render(
      <ConfirmAction
        question="Delete everything?"
        actionLabel="Delete"
        onConfirm={onConfirm}
        onCancel={onCancel}
      />,
    );

    expect(screen.getByText("Delete everything?")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Keep" }));
    expect(onCancel).toHaveBeenCalledTimes(1);
    expect(onConfirm).not.toHaveBeenCalled();

    await userEvent.click(screen.getByRole("button", { name: "Delete" }));
    expect(onConfirm).toHaveBeenCalledTimes(1);
  });

  it("keeps the confirm disarmed while its extra condition holds", () => {
    render(
      <ConfirmAction
        question="Delete everything?"
        actionLabel="Delete"
        confirmDisabled
        onConfirm={vi.fn()}
        onCancel={vi.fn()}
      />,
    );

    expect(screen.getByRole("button", { name: "Delete" })).toBeDisabled();
    // Backing out never needs the arming condition.
    expect(screen.getByRole("button", { name: "Keep" })).toBeEnabled();
  });

  it("disables both buttons while the action is in flight", () => {
    render(
      <ConfirmAction
        question="Delete everything?"
        actionLabel="Delete"
        busy
        onConfirm={vi.fn()}
        onCancel={vi.fn()}
      />,
    );

    expect(screen.getByRole("button", { name: "Delete" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Keep" })).toBeDisabled();
  });
});

describe("FailureNotice", () => {
  it("announces the detail and its recovery as one alert on two lines", () => {
    render(<FailureNotice detail="the daemon refused" recovery="grant the capability, then retry" />);

    const alert = screen.getByRole("alert");
    expect(alert).toHaveTextContent("the daemon refused");
    expect(alert).toHaveTextContent("grant the capability, then retry");
    expect(screen.getByText("the daemon refused")).not.toBe(
      screen.getByText("grant the capability, then retry"),
    );
  });

  it("renders without a recovery line when there is none", () => {
    render(<FailureNotice detail="the request failed" />);
    expect(screen.getByRole("alert")).toHaveTextContent("the request failed");
  });
});

describe("SuccessNotice", () => {
  it("reports an outcome as a status, not an alert", () => {
    render(<SuccessNotice>Removed 9 items.</SuccessNotice>);
    expect(screen.getByRole("status")).toHaveTextContent("Removed 9 items.");
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });
});
