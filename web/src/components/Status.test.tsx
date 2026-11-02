import { render, screen } from "@testing-library/react";
import { Status } from "./Status";

describe("Status", () => {
  it("announces the connection state without the ticking update time", () => {
    render(<Status connection="live" builtAt={Date.UTC(2026, 10, 2, 1, 2, 3)} />);
    expect(screen.getByRole("status")).toHaveTextContent(/^Live$/);
    expect(screen.getByText(/^Updated/)).toBeVisible();
    expect(screen.getByRole("status")).not.toContainElement(screen.getByText(/^Updated/));
  });

  it("names every connection state", () => {
    for (const [connection, label] of [
      ["connecting", "Connecting"],
      ["reconnecting", "Reconnecting"],
      ["database-offline", "Database offline"],
    ] as const) {
      const { unmount } = render(<Status connection={connection} builtAt={null} />);
      expect(screen.getByRole("status")).toHaveTextContent(label);
      unmount();
    }
  });
});
