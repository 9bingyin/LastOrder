import { expect, it, spyOn } from "bun:test";
import { generateNickname } from "./nickname";

it("昵称独立随机组合形容词与水果，索引边界不产生空值", () => {
  const random = spyOn(Math, "random");
  try {
    const names = [
      [0, 0],
      [0.999999, 0],
      [0, 0.999999],
      [0.999999, 0.999999],
    ].map(([adjective, fruit]) => {
      random
        .mockReturnValueOnce(adjective ?? 0)
        .mockReturnValueOnce(fruit ?? 0);
      return generateNickname();
    });
    for (const name of names) {
      expect(name).toMatch(/^.+的.+$/);
      expect(name.length).toBeLessThanOrEqual(32);
      expect(name).not.toContain("undefined");
    }
    const parts = names.map((name) => name.split("的"));
    expect(parts[1]?.[0]).not.toBe(parts[0]?.[0]);
    expect(parts[1]?.[1]).toBe(parts[0]?.[1]);
    expect(parts[2]?.[0]).toBe(parts[0]?.[0]);
    expect(parts[2]?.[1]).not.toBe(parts[0]?.[1]);
    expect(parts[3]?.[0]).toBe(parts[1]?.[0]);
    expect(parts[3]?.[1]).toBe(parts[2]?.[1]);
  } finally {
    random.mockRestore();
  }
});
