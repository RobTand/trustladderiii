import { render, screen, within } from "@testing-library/react";
import { BoardColumn } from "./BoardColumn";
import { card } from "../fixtures";

describe("BoardColumn", () => {
  it("heads the column with its label and count and lists cards in id order", () => {
    render(
      <BoardColumn
        column="Making"
        label="Making"
        now={1_000}
        cards={[card({ id: 5 }), card({ id: 2 }), card({ id: 9 })]}
      />,
    );
    const region = screen.getByRole("region", { name: "Making" });
    expect(within(region).getByText("3")).toBeVisible();
    expect(within(region).getAllByRole("article").map((a) => a.getAttribute("data-card"))).toEqual(["2", "5", "9"]);
  });

  it("lists Done newest first", () => {
    render(
      <BoardColumn
        column="Done"
        label="Done"
        now={1_000}
        cards={[
          card({ id: 1, column: "Done", phase_since_ms: 100 }),
          card({ id: 2, column: "Done", phase_since_ms: 300 }),
          card({ id: 3, column: "Done", phase_since_ms: 200 }),
        ]}
      />,
    );
    expect(screen.getAllByRole("article").map((a) => a.getAttribute("data-card"))).toEqual(["2", "3", "1"]);
  });

  it("says when a column is empty", () => {
    render(<BoardColumn column="Backlog" label="Backlog" now={1_000} cards={[]} />);
    const region = screen.getByRole("region", { name: "Backlog" });
    expect(within(region).getByText("0")).toBeVisible();
    expect(within(region).getByText("Nothing here")).toBeVisible();
  });
});
