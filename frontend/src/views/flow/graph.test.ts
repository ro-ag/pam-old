import { describe, expect, it } from "vitest";
import { afterMergeDefinition } from "../../fixtures";
import { actionSummary, addStep, deleteStep, layoutSteps, renameStep, stepEdges } from "./graph";

describe("flow graph model", () => {
  it("lays the fixture steps out by dependency depth", () => {
    const positions = layoutSteps(afterMergeDefinition.steps);
    expect(positions.size).toBe(2);
    expect(positions.get("observe-revision")).toEqual({ x: 0, y: 0 });
    expect(positions.get("verify-worktree")).toEqual({ x: 320, y: 0 });
  });

  it("derives one edge per resolvable dependency", () => {
    expect(stepEdges(afterMergeDefinition.steps)).toEqual([
      { id: "observe-revision->verify-worktree", source: "observe-revision", target: "verify-worktree" },
    ]);
  });

  it("keeps a dependency cycle bounded instead of recursing", () => {
    const steps = [
      { ...afterMergeDefinition.steps[0], id: "a", depends_on: ["b"] },
      { ...afterMergeDefinition.steps[0], id: "b", depends_on: ["a"] },
    ];
    expect(layoutSteps(steps).size).toBe(2);
  });

  it("renames a step and every reference to it", () => {
    const renamed = renameStep(afterMergeDefinition, "observe-revision", "observe-head");
    expect(renamed.steps[0].id).toBe("observe-head");
    expect(renamed.steps[1].depends_on).toEqual(["observe-head"]);
    expect(renamed.steps[1].condition).toEqual({ kind: "succeeded", step: "observe-head" });
  });

  it("deletes a step and strips it from every dependent", () => {
    const remaining = deleteStep(afterMergeDefinition, "observe-revision");
    expect(remaining.steps.map((step) => step.id)).toEqual(["verify-worktree"]);
    expect(remaining.steps[0].depends_on).toEqual([]);
    expect(remaining.steps[0].condition).toEqual({ kind: "always" });
  });

  it("adds steps under fresh ids", () => {
    const added = addStep(afterMergeDefinition);
    expect(added.id).toBe("step-3");
    expect(added.definition.steps).toHaveLength(3);
    expect(addStep(added.definition).id).toBe("step-4");
  });

  it("summarizes both action kinds", () => {
    expect(actionSummary(afterMergeDefinition.steps[0].action)).toBe("git rev-parse --verify HEAD");
    expect(actionSummary({
      type: "connector",
      connector: "github",
      capability: "issues.read",
      resource: { kind: "repo", id: "pam" },
    })).toBe("github · issues.read · repo/pam");
  });
});
