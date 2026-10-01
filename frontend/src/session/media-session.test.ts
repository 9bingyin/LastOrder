import { afterEach, beforeEach, expect, it, spyOn } from "bun:test";
import * as connector from "../media/connect";
import { defaultProfile } from "../media/quality";
import { deferred, fakeMedia } from "../test-support/media";
import { MediaSession } from "./media-session";

let fetchSpy: import("bun:test").Mock<
  (...args: Parameters<typeof fetch>) => ReturnType<typeof fetch>
>;
let connectSpy: ReturnType<typeof spyOn<typeof connector, "connectMedia">>;
let owner: MediaSession;
let displayed: MediaStream | null;
let errors: string[];
const originalDevices = Object.getOwnPropertyDescriptor(
  navigator,
  "mediaDevices",
);
const payload = (id: string) => ({
  id,
  shareId: `share-${id}`,
  generation: "generation",
  rtcPeerId: `rtc-${id}`,
  clientId: "page",
  audio: false,
  state: "preparing",
});
const flush = () => new Promise<void>((resolve) => setImmediate(resolve));

beforeEach(() => {
  fetchSpy = spyOn(globalThis, "fetch");
  connectSpy = spyOn(connector, "connectMedia");
  displayed = null;
  errors = [];
  owner = new MediaSession({
    capture: (stream) => {
      displayed = stream;
    },
    watch: (stream) => {
      displayed = stream;
    },
    error: (message) => errors.push(message),
  });
});
afterEach(() => {
  owner.clearCapture();
  owner.clearWatching();
  fetchSpy.mockRestore();
  connectSpy.mockRestore();
  if (originalDevices)
    Object.defineProperty(navigator, "mediaDevices", originalDevices);
  else Reflect.deleteProperty(navigator, "mediaDevices");
});

it.each(["picker", "create", "connect"] as const)(
  "捕获在 %s 阶段失效，迟到结果必须释放且不能恢复画面",
  async (stage) => {
    const source = fakeMedia();
    const entered = deferred<void>();
    const release = deferred<void>();
    const deleted: string[] = [];
    Object.defineProperty(navigator, "mediaDevices", {
      configurable: true,
      value: {
        getDisplayMedia: async () => {
          if (stage === "picker") {
            entered.resolve();
            await release.promise;
          }
          return source.stream;
        },
      },
    });
    fetchSpy.mockImplementation(async (input, init) => {
      if (init?.method === "DELETE") {
        deleted.push(String(input));
        return Response.json({});
      }
      if (stage === "create") {
        entered.resolve();
        await release.promise;
      }
      return Response.json(payload("old"));
    });
    connectSpy.mockImplementation(async () => {
      if (stage === "connect") {
        entered.resolve();
        await release.promise;
      }
      return source.pc;
    });
    const operation = owner.startCapture(
      "test-token",
      "page",
      defaultProfile,
      () => {},
    );
    await entered.promise;
    owner.clearCapture();
    if (stage !== "picker") expect(source.live).toBe(false);
    release.resolve();
    await operation;
    expect(source.live).toBe(false);
    expect(displayed).toBeNull();
    expect(owner.capture).toBeNull();
    expect(owner.pendingCapture).toBeNull();
    expect(errors).toEqual([]);
    expect(deleted).toEqual(
      stage === "picker" ? [] : ["/api/v1/shares/share-old"],
    );
    if (stage === "connect") expect(source.closed).toBe(true);
  },
);

it.each(["create", "connect"] as const)(
  "观看在 %s 阶段失效，迟到结果必须取消服务端订阅",
  async (stage) => {
    const source = fakeMedia();
    const entered = deferred<void>();
    const release = deferred<void>();
    const deleted: string[] = [];
    fetchSpy.mockImplementation(async (input, init) => {
      if (init?.method === "DELETE") {
        deleted.push(String(input));
        return Response.json({});
      }
      if (stage === "create") {
        entered.resolve();
        await release.promise;
      }
      return Response.json(payload("old"));
    });
    connectSpy.mockImplementation(
      async (_token, _page, _media, _stream, onStream) => {
        entered.resolve();
        await release.promise;
        onStream(source.stream);
        return source.pc;
      },
    );
    const operation = owner.startWatching("test-token", "page", {
      id: "share",
      generation: "generation",
    });
    await entered.promise;
    owner.clearWatching();
    release.resolve();
    await operation;
    expect(owner.watching).toBeNull();
    expect(displayed).toBeNull();
    expect(deleted).toEqual(["/api/v1/subscriptions/old"]);
    if (stage === "connect") expect(source.closed).toBe(true);
  },
);

it.each([
  ["成功", false],
  ["失败", true],
] as const)("旧观看延迟%s 不能覆盖或清理新会话", async (_label, fails) => {
  const old = fakeMedia();
  const current = fakeMedia();
  const entered = deferred<void>();
  const release = deferred<void>();
  let creates = 0;
  const deleted: string[] = [];
  fetchSpy.mockImplementation(async (input, init) => {
    if (init?.method === "DELETE") {
      deleted.push(String(input));
      return Response.json({});
    }
    return Response.json(payload(creates++ === 0 ? "old" : "new"));
  });
  connectSpy.mockImplementation(
    async (_token, _page, media, _stream, onStream, onFailed) => {
      if (media.id === "old") {
        entered.resolve();
        await release.promise;
        onStream(old.stream);
        onFailed();
        if (fails) throw new Error("old failure");
        return old.pc;
      }
      onStream(current.stream);
      return current.pc;
    },
  );
  const operation = owner.startWatching("test-token", "page", {
    id: "share",
    generation: "generation",
  });
  await entered.promise;
  owner.clearWatching();
  await owner.startWatching("test-token", "page", {
    id: "share",
    generation: "generation",
  });
  release.resolve();
  await operation;
  expect(owner.watching?.media.id).toBe("new");
  expect(displayed).toBe(current.stream);
  expect(current.closed).toBe(false);
  expect(errors).toEqual([]);
  expect(deleted).toEqual(["/api/v1/subscriptions/old"]);
  if (!fails) expect(old.closed).toBe(true);
  await flush();
});

it("画质请求失败时恢复已应用参数，队列仍可继续", async () => {
  const source = fakeMedia();
  Object.defineProperty(navigator, "mediaDevices", {
    configurable: true,
    value: { getDisplayMedia: async () => source.stream },
  });
  fetchSpy.mockResolvedValueOnce(Response.json(payload("capture")));
  connectSpy.mockResolvedValue(source.pc);
  await owner.startCapture(
    "test-token",
    "page",
    defaultProfile,
    () => {},
  );
  fetchSpy.mockResolvedValueOnce(
    Response.json({ error: { message: "quality rejected" } }, { status: 400 }),
  );
  const next = {
    ...defaultProfile,
    height: 720,
    fps: 60,
    bitrate: 4_000_000,
  } as const;
  await expect(
    owner.updateQuality("test-token", "page", next, defaultProfile),
  ).rejects.toThrow("quality rejected");
  expect(source.parameters().encodings[0]).toMatchObject({
    maxBitrate: 6_000_000,
    maxFramerate: 30,
    scaleResolutionDownBy: 1,
  });
  expect(source.track.getConstraints().height).toEqual({
    ideal: 1080,
    max: 1080,
  });
  fetchSpy.mockResolvedValueOnce(Response.json({}));
  expect(
    await owner.updateQuality("test-token", "page", next, defaultProfile),
  ).toBe(true);
  expect(source.parameters().encodings[0]).toMatchObject({
    maxBitrate: 4_000_000,
    maxFramerate: 60,
    scaleResolutionDownBy: 1.5,
  });
  const update = fetchSpy.mock.calls.at(-1);
  expect(update?.[0]).toBe("/api/v1/shares/share-capture/quality");
  expect(update?.[1]?.method).toBe("POST");
  expect(JSON.parse(String(update?.[1]?.body))).toEqual({
    mode: "original",
    height: 720,
    fps: 60,
    bitrate: 4_000_000,
  });
});

it("当前轨道结束会停止分享，旧轨道迟到事件不能停止新捕获", async () => {
  const old = fakeMedia();
  const current = fakeMedia();
  const stopped = deferred<void>();
  let picks = 0;
  let creates = 0;
  let ended = 0;
  const deleted: string[] = [];
  Object.defineProperty(navigator, "mediaDevices", {
    configurable: true,
    value: {
      getDisplayMedia: async () =>
        picks++ === 0 ? old.stream : current.stream,
    },
  });
  fetchSpy.mockImplementation(async (input, init) => {
    if (init?.method === "DELETE") {
      deleted.push(String(input));
      return Response.json({});
    }
    return Response.json(payload(creates++ === 0 ? "old" : "new"));
  });
  connectSpy.mockImplementation(async (_token, _page, media) =>
    media.id === "old" ? old.pc : current.pc,
  );
  const onEnded = () => {
    ended++;
    void owner
      .stopCapture("test-token", "page")
      .then(stopped.resolve, stopped.reject);
  };
  await owner.startCapture(
    "test-token",
    "page",
    defaultProfile,
    onEnded,
  );
  await owner.stopCapture("test-token", "page");
  await owner.startCapture(
    "test-token",
    "page",
    defaultProfile,
    onEnded,
  );
  old.track.dispatchEvent(new Event("ended"));
  expect(ended).toBe(0);
  expect(owner.capture?.media.id).toBe("new");
  expect(displayed).toBe(current.stream);
  expect(current.live).toBe(true);
  current.track.dispatchEvent(new Event("ended"));
  await stopped.promise;
  expect(ended).toBe(1);
  expect(owner.capture).toBeNull();
  expect(displayed).toBeNull();
  expect(current.live).toBe(false);
  expect(current.closed).toBe(true);
  expect(deleted).toEqual([
    "/api/v1/shares/share-old",
    "/api/v1/shares/share-new",
  ]);
});
