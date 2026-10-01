import { describe, expect, it } from "bun:test";
import {
  defaultProfile,
  displayOptions,
  encodingBudget,
  videoConstraints,
  videoProfileSchema,
} from "./quality";

describe("共享画质与音频", () => {
  it("默认 1080p、30 fps、6 Mbps", () => {
    expect(defaultProfile).toEqual({
      mode: "original",
      height: 1080,
      fps: 30,
      bitrate: 6_000_000,
    });
    expect(videoConstraints(defaultProfile)).toEqual({
      width: { ideal: 1920, max: 1920 },
      height: { ideal: 1080, max: 1080 },
      frameRate: { ideal: 30, max: 30 },
    });
  });

  it("原画保留手动上限，自动连续分配预算与音频余量", () => {
    const cases = [
      {
        mode: "original",
        audio: true,
        network: 1_234_567,
        expected: { video: 6_000_000, audio: 128_000 },
      },
      {
        mode: "auto",
        audio: true,
        network: 1_234_567,
        expected: { video: 1_106_567, audio: 128_000 },
      },
      {
        mode: "auto",
        audio: true,
        network: 64_000,
        expected: { video: 40_000, audio: 24_000 },
      },
      {
        mode: "auto",
        audio: false,
        network: 64_000_000,
        expected: { video: 6_000_000, audio: 0 },
      },
    ] as const;
    for (const row of cases)
      expect(
        encodingBudget(
          { ...defaultProfile, mode: row.mode },
          row.audio,
          row.network,
        ),
      ).toEqual(row.expected);
  });

  it("请求共享声音并关闭语音处理", () => {
    expect(displayOptions(defaultProfile, true)).toMatchObject({
      systemAudio: "include",
      windowAudio: "window",
      audio: {
        echoCancellation: false,
        noiseSuppression: false,
        autoGainControl: false,
        channelCount: { ideal: 2 },
      },
    });
    expect(displayOptions(defaultProfile, false)).toMatchObject({
      audio: false,
      systemAudio: "exclude",
    });
  });

  it("校验可编辑画质", () => {
    expect(
      videoProfileSchema.parse({ height: 720, fps: 60, bitrate: 4_000_000 }),
    ).toEqual({ mode: "original", height: 720, fps: 60, bitrate: 4_000_000 });
    for (const invalid of [
      { ...defaultProfile, mode: "preset" },
      { ...defaultProfile, height: 999 },
      { ...defaultProfile, fps: 0 },
      { ...defaultProfile, fps: 120 },
      { ...defaultProfile, bitrate: Number.NaN },
      { ...defaultProfile, bitrate: 31_000_000 },
    ]) {
      expect(videoProfileSchema.safeParse(invalid).success).toBe(false);
    }
  });
});
