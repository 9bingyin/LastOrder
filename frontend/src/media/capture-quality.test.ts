import { expect, it, spyOn } from "bun:test";
import { CaptureQuality } from "./capture-quality";
import { defaultProfile } from "./quality";
import { deferred, fakeMedia } from "../test-support/media";

it("在途参数更新不重叠，连续预算最终采用最新值", async () => {
  const source = fakeMedia();
  const entered = deferred<void>();
  const release = deferred<void>();
  const latest = deferred<void>();
  const original = source.sender.setParameters;
  let active = 0;
  let maximum = 0;
  const applied: number[] = [];
  const setter = spyOn(source.sender, "setParameters").mockImplementation(
    async (parameters) => {
      active++;
      maximum = Math.max(maximum, active);
      try {
        if (applied.length === 0) {
          entered.resolve();
          await release.promise;
        }
        await original(parameters);
        applied.push(parameters.encodings[0]?.maxBitrate ?? 0);
        if (parameters.encodings[0]?.maxBitrate === 4_321_987) latest.resolve();
      } finally {
        active--;
      }
    },
  );
  const quality = new CaptureQuality(
    source.pc,
    source.stream,
    { ...defaultProfile, mode: "auto" },
    () => {},
  );
  try {
    const initial = quality.applyProfile({ ...defaultProfile, mode: "auto" });
    await entered.promise;
    quality.setBudget(1_234_567);
    quality.setBudget(4_321_987);
    release.resolve();
    await initial;
    await latest.promise;
    await quality.applyProfile(defaultProfile);
    expect(maximum).toBe(1);
    expect(applied).not.toContain(1_234_567);
    expect(applied).toContain(4_321_987);
    expect(source.parameters().encodings[0]).toMatchObject({
      maxBitrate: 6_000_000,
      maxFramerate: 30,
    });
    expect(source.parameters().degradationPreference).toBe(
      "maintain-resolution",
    );
    expect(source.track.contentHint).toBe("text");
  } finally {
    release.resolve();
    quality.dispose();
    setter.mockRestore();
  }
});

it("捕获约束或发送器失败会向调用者报错，但不破坏后续更新", async () => {
  const source = fakeMedia();
  const constraints = spyOn(
    source.track,
    "applyConstraints",
  ).mockRejectedValueOnce(new Error("capture denied"));
  const setter = spyOn(source.sender, "setParameters");
  const quality = new CaptureQuality(
    source.pc,
    source.stream,
    defaultProfile,
    () => {},
  );
  try {
    await expect(quality.applyProfile(defaultProfile)).rejects.toThrow(
      "capture denied",
    );
    setter.mockRejectedValueOnce(new Error("sender denied"));
    await expect(quality.applyProfile(defaultProfile)).rejects.toThrow(
      "sender denied",
    );
    await quality.applyProfile({
      ...defaultProfile,
      fps: 60,
      bitrate: 4_000_000,
    });
    expect(source.parameters().encodings[0]).toMatchObject({
      maxBitrate: 4_000_000,
      maxFramerate: 60,
    });
  } finally {
    quality.dispose();
    constraints.mockRestore();
    setter.mockRestore();
  }
});

it.each([
  ["仍在分享", false],
  ["已释放", true],
] as const)(
  "启动统计迟到：%s 会话保持正确的编码状态",
  async (_label, disposed) => {
    const source = fakeMedia();
    const entered = deferred<void>();
    const release = deferred<RTCStatsReport>();
    const updated = deferred<void>();
    const stats = spyOn(source.peer, "getStats").mockImplementation(
      async () => {
        entered.resolve();
        return release.promise;
      },
    );
    const original = source.sender.setParameters;
    const setter = spyOn(source.sender, "setParameters").mockImplementation(
      async (parameters) => {
        await original(parameters);
        if (parameters.degradationPreference === "balanced") updated.resolve();
      },
    );
    const errors: unknown[] = [];
    const quality = new CaptureQuality(
      source.pc,
      source.stream,
      { ...defaultProfile, mode: "auto" },
      (error) => errors.push(error),
    );
    try {
      await quality.applyProfile({ ...defaultProfile, mode: "auto" });
      expect(source.parameters().degradationPreference).toBe(
        "maintain-resolution",
      );
      await entered.promise;
      if (disposed) quality.dispose();
      release.resolve(
        new Map([
          [
            "video",
            {
              id: "video",
              timestamp: 0,
              type: "outbound-rtp",
              kind: "video",
              framesEncoded: 5,
            },
          ],
        ]),
      );
      if (!disposed) {
        await updated.promise;
        expect(source.parameters().degradationPreference).toBe("balanced");
      } else {
        await new Promise<void>((resolve) => setImmediate(resolve));
        expect(source.parameters().degradationPreference).toBe(
          "maintain-resolution",
        );
        quality.setBudget(1_234_567);
        await quality.applyProfile({
          ...defaultProfile,
          fps: 60,
          bitrate: 4_000_000,
        });
        expect(source.parameters().encodings[0]).toMatchObject({
          maxBitrate: 6_000_000,
          maxFramerate: 30,
        });
        expect(source.track.getConstraints().frameRate).toEqual({
          ideal: 30,
          max: 30,
        });
      }
      expect(errors).toEqual([]);
    } finally {
      quality.dispose();
      stats.mockRestore();
      setter.mockRestore();
    }
  },
);
