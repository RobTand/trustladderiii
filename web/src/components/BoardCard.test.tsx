import { render, screen, within } from "@testing-library/react";
import { BoardCard } from "./BoardCard";
import { card } from "../fixtures";

const MINUTE = 60_000;

describe("BoardCard", () => {
  it("shows the request, its stage, role and model, counters and time in stage", () => {
    render(
      <BoardCard
        now={1_000 + 75 * MINUTE}
        card={card({
          id: 2,
          issue: 128,
          repo: "RobTand/prismaquant",
          title: "Quantize the attention layers",
          role: "Reviewer",
          model: "claude-fable-5-1 (high)",
          attempt: 2,
          round: 1,
          credit: 3,
          retries: { plan: 1, audit: 0, run_failures: 2, effect_failures: 0 },
          waiting_on: "the review to finish",
          phase: "Reviewing",
        })}
      />,
    );
    const article = screen.getByRole("article", {
      name: "RobTand/prismaquant#128: Quantize the attention layers",
    });
    const view = within(article);
    expect(view.getByText("prismaquant")).toBeVisible();
    expect(view.getByText("#128")).toBeVisible();
    expect(view.getByText("Reviewing")).toBeVisible();
    expect(view.getByText("the review to finish")).toBeVisible();
    expect(view.getByText("Reviewer · claude-fable-5-1 (high)")).toBeVisible();
    expect(view.getByText("Attempt 2 · Round 1")).toBeVisible();
    expect(view.getByText("3 retries")).toBeVisible();
    expect(view.getByLabelText("Credit")).toHaveTextContent("3");
    expect(view.getByLabelText("Time in stage")).toHaveTextContent("1h 15m");
  });

  it("names every badge and the ending of a finished request", () => {
    render(
      <BoardCard
        now={1_000}
        card={card({
          id: 1,
          column: "Done",
          phase: "Closed: merged",
          ending: "Merged",
          role: null,
          model: null,
          badges: ["Paused", "Incident", "MakerBlocked", "NeedsRob", "WaitingForCapacity"],
        })}
      />,
    );
    for (const label of ["Paused", "Incident", "Maker blocked", "Needs Rob", "Waiting for capacity", "Merged"]) {
      expect(screen.getByText(label)).toBeVisible();
    }
    expect(screen.queryByText(/·/)).toBeNull();
  });

  it("renders hostile titles as text", () => {
    render(<BoardCard now={1_000} card={card({ id: 1, title: "<img src=x onerror=alert(1)>" })} />);
    expect(screen.getByText("<img src=x onerror=alert(1)>")).toBeVisible();
    expect(document.querySelector("img")).toBeNull();
  });
});
