import { afterEach, beforeEach, expect, it, jest, spyOn } from "bun:test";
import { deferred, fakeMedia } from "../test-support/media";
import { startSampling } from "./sampling";
import type { Sample } from "./sampling";
import type { DebugMedia } from "./diagnostics";

let fetchSpy: import("bun:test").Mock<
  (...args: Parameters<typeof fetch>) => ReturnType<typeof fetch>
>;
let stop = () => {};
const flush = () => new Promise<void>((resolve) => setImmediate(resolve));
const raw = {
  version: "1",
  snapshot: { joinCode: "room-credential", room: { id: "room-credential" } },
  endpointAddressRaw: "address",
  boundSockets: [],
  connections: [
    {
      remoteId: "remote",
      statsRaw: "metric 42 test-token room-credential",
      paths: [{ statsRaw: "path 99 room-credential" }],
    },
  ],
  localWebRtc: {},
  actor: {},
};
beforeEach(() => {
  fetchSpy = spyOn(globalThis, "fetch");
  jest.useFakeTimers();
});
afterEach(() => {
  stop();
  fetchSpy.mockRestore();
  jest.useRealTimers();
});

it("暂停取消请求，迟到结果不会发布或重新开始采样", async () => {
  const response = deferred<Response>();
  const entered = deferred<void>();
  let signal: AbortSignal | null | undefined;
  fetchSpy.mockImplementation(async (_input, init) => {
    signal = init?.signal;
    entered.resolve();
    return response.promise;
  });
  const published: Sample[] = [];
  stop = startSampling(
    "test-token",
    () => [],
    () => ({}),
    (sample) => published.push(sample),
  );
  await entered.promise;
  stop();
  expect(signal?.aborted).toBe(true);
  response.resolve(Response.json(raw));
  await flush();
  jest.advanceTimersByTime(2000);
  await flush();
  expect(published).toEqual([]);
  expect(fetchSpy.mock.calls).toHaveLength(1);
});

it("撤销在途媒体会过滤旧结果，展示与复制共用已脱敏的完整样本", async () => {
  const source = fakeMedia();
  const entered = deferred<void>();
  const result = deferred<RTCStatsReport>();
  const published = deferred<Sample>();
  const stats = spyOn(source.peer, "getStats").mockImplementation(async () => {
    entered.resolve();
    return result.promise;
  });
  let resources: DebugMedia[] = [
    { id: "old", kind: "watch", pc: source.pc, stream: null },
  ];
  fetchSpy.mockResolvedValue(Response.json(raw));
  try {
    stop = startSampling(
      "test-token",
      () => resources,
      () => ({ ordinary: "browser-marker" }),
      published.resolve,
    );
    await entered.promise;
    resources = [];
    result.resolve(new Map());
    const sample = await published.promise;
    expect(sample.media).toEqual([]);
    const text = JSON.stringify(sample);
    expect(text).not.toContain("test-token");
    expect(text).not.toContain("room-credential");
    expect(sample.backend?.connections[0]?.statsRaw).toBe(
      "metric 42 [redacted] [redacted]",
    );
    expect(sample.backend?.connections[0]?.paths[0]?.statsRaw).toBe(
      "path 99 [redacted]",
    );
    expect(sample.browser).toEqual({ ordinary: "browser-marker" });
  } finally {
    result.resolve(new Map());
    stats.mockRestore();
  }
});

it.each([
  ["媒体失败", false],
  ["后端失败", true],
] as const)("%s 不会丢弃另一侧的可用诊断", async (_label, backendFails) => {
  const source = fakeMedia();
  const published = deferred<Sample>();
  const stats = spyOn(source.peer, "getStats");
  if (backendFails) {
    fetchSpy.mockResolvedValue(new Response("unavailable", { status: 503 }));
    stats.mockResolvedValue(
      new Map([
        [
          "video",
          {
            id: "video",
            type: "inbound-rtp",
            timestamp: 0,
            bytesReceived: 42,
          },
        ],
      ]),
    );
  } else {
    fetchSpy.mockResolvedValue(Response.json(raw));
    stats.mockRejectedValue(new Error("stats unavailable"));
  }
  try {
    stop = startSampling(
      "test-token",
      () => [{ id: "watch", kind: "watch", pc: source.pc, stream: null }],
      () => ({}),
      published.resolve,
    );
    const sample = await published.promise;
    if (backendFails) {
      expect(sample.backend).toBeNull();
      expect(sample.failure).toContain("503");
      expect(sample.media[0]).toMatchObject({
        stats: [{ bytesReceived: 42 }],
        statsError: null,
      });
    } else {
      expect(sample.backend?.version).toBe("1");
      expect(sample.failure).toBeNull();
      expect(sample.media[0]).toMatchObject({
        statsError: "stats unavailable",
      });
    }
  } finally {
    stats.mockRestore();
  }
});
