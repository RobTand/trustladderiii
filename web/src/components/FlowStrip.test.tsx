import { render, screen } from "@testing-library/react";
import { FlowStrip } from "./FlowStrip";
import { board, card } from "../fixtures";

describe("FlowStrip", () => {
  it("shows counts per stage, items in flight and recent merges", () => {
    const { flow } = board([
      card({ id: 1, column: "Intake" }),
      card({ id: 2, column: "Making" }),
      card({ id: 3, column: "Making" }),
      card({ id: 4, column: "Done" }),
    ]);
    render(<FlowStrip flow={{ ...flow, merged_last_hour: 2, merged_last_day: 11 }} />);
    const value = (label: string) => screen.getByText(label, { selector: "dt" }).nextElementSibling;
    expect(value("Intake")).toHaveTextContent("1");
    expect(value("Making")).toHaveTextContent("2");
    expect(value("Needs Rob")).toHaveTextContent("0");
    expect(value("Done")).toHaveTextContent("1");
    expect(value("In flight")).toHaveTextContent("3");
    expect(value("Merged, last hour")).toHaveTextContent("2");
    expect(value("Merged, last day")).toHaveTextContent("11");
    // A dl may group each dt and dd in one div, and nothing deeper.
    for (const term of document.querySelectorAll("dt")) {
      expect(term.parentElement?.parentElement?.tagName).toBe("DL");
    }
  });
});
