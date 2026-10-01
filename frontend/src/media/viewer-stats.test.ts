import { describe, expect, it } from "bun:test";
import { videoSample, viewerStats } from "./viewer-stats";
import type { VideoSample } from "./viewer-stats";

const sample: VideoSample = {
  id: "video",
  timestamp: 1000,
  bytes: 100_000,
  width: 1280,
  height: 720,
  dropped: 3,
};

describe("观看视频统计", () => {
  it("只读取视频接收流，不把音频码率和丢包当作视频丢帧", () => {
    const report = new Map<string, unknown>([
      [
        "audio",
        {
          type: "inbound-rtp",
          kind: "audio",
          id: "audio",
          timestamp: 2000,
          bytesReceived: 900_000,
        },
      ],
      [
        "outgoing",
        {
          type: "outbound-rtp",
          kind: "video",
          id: "outgoing",
          timestamp: 2000,
          bytesSent: 900_000,
        },
      ],
      [
        "video",
        {
          type: "inbound-rtp",
          kind: "video",
          id: "video",
          timestamp: 2000,
          bytesReceived: 350_000,
          frameWidth: 1280,
          frameHeight: 720,
          framesDropped: 7,
          packetsLost: 90,
        },
      ],
    ]);
    for (const key of ["audio", "outgoing"])
      expect(videoSample(new Map([[key, report.get(key)]]))).toBeNull();
    for (const rows of [Array.from(report), Array.from(report).reverse()])
      expect(videoSample(new Map(rows))).toEqual({
        ...sample,
        timestamp: 2000,
        bytes: 350_000,
        dropped: 7,
      });
  });

  it("按真实采样间隔计算接收码率", () => {
    expect(
      viewerStats(
        { ...sample, timestamp: 3000, bytes: 600_000, dropped: 9 },
        sample,
        1920,
        1080,
      ),
    ).toEqual({ bitrate: 2_000_000, resolution: "1280×720", dropped: 9 });
  });

  it("初次采样和浏览器缺失字段不显示伪造的零值", () => {
    const current = videoSample(
      new Map([
        [
          "video",
          { type: "inbound-rtp", kind: "video", id: "video", timestamp: 1000 },
        ],
      ]),
    );
    expect(viewerStats(current, null, 1920, 1080)).toEqual({
      bitrate: null,
      resolution: "1920×1080",
      dropped: null,
    });
    for (const partial of [
      { ...sample, height: null },
      { ...sample, width: null },
    ])
      expect(viewerStats(partial, null, 1920, 1080).resolution).toBe(
        "1920×1080",
      );
  });

  it("流切换、计数器回退和重复时间戳重置差分", () => {
    for (const current of [
      { ...sample, id: "new-video", timestamp: 2000 },
      { ...sample, bytes: 50_000, timestamp: 2000 },
      { ...sample, bytes: 200_000 },
      { ...sample, timestamp: 500 },
    ])
      expect(viewerStats(current, sample, 0, 0).bitrate).toBeNull();
  });

  it("停止传输时码率为零，但未知的统计仍保持未知", () => {
    expect(
      viewerStats({ ...sample, timestamp: 2000 }, sample, 0, 0).bitrate,
    ).toBe(0);
    expect(viewerStats(null, null, 0, 0)).toEqual({
      bitrate: null,
      resolution: null,
      dropped: null,
    });
  });

  it("无效和无限统计数值不能进入界面", () => {
    const current = videoSample(
      new Map([
        [
          "video",
          {
            type: "inbound-rtp",
            kind: "video",
            id: "video",
            timestamp: 2000,
            bytesReceived: Infinity,
            frameWidth: -1,
            framesDropped: NaN,
          },
        ],
      ]),
    );
    expect(viewerStats(current, sample, 0, 0)).toEqual({
      bitrate: null,
      resolution: null,
      dropped: null,
    });
  });
});
