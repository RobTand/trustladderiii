import { filterCards } from "./filter";
import { card } from "./fixtures";

describe("filterCards", () => {
  const cards = [
    card({ id: 1, repo: "RobTand/prismabuild", issue: 812, title: "Shard the GPU queue" }),
    card({ id: 2, repo: "RobTand/tessera", issue: 12, title: "Fix tile seams" }),
    card({ id: 3, repo: "RobTand/tessera", issue: 81, title: "Queue tiles lazily" }),
  ];
  const ids = (repo: string | null, query: string) => filterCards(cards, { repo, query }).map((c) => c.id);

  it("filters by repo", () => {
    expect(ids("RobTand/tessera", "")).toEqual([2, 3]);
    expect(ids(null, "")).toEqual([1, 2, 3]);
  });

  it("searches issue numbers exactly and titles by words", () => {
    expect(ids(null, "#81")).toEqual([3]);
    expect(ids(null, "81")).toEqual([3]);
    expect(ids(null, "queue")).toEqual([1, 3]);
    expect(ids("RobTand/tessera", "QUEUE")).toEqual([3]);
  });
});
