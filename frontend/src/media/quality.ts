import { z } from "zod";

export const videoProfileSchema = z.object({
  mode: z.enum(["original", "auto"]).default("original"),
  height: z.union([
    z.literal(480),
    z.literal(720),
    z.literal(1080),
    z.literal(1440),
    z.literal(2160),
  ]),
  fps: z.number().int().min(15).max(60),
  bitrate: z.number().int().min(500_000).max(30_000_000),
});
export type VideoProfile = z.infer<typeof videoProfileSchema>;
export const defaultProfile = {
  mode: "original",
  height: 1080,
  fps: 30,
  bitrate: 6_000_000,
} satisfies VideoProfile;

export function videoConstraints(profile: VideoProfile): MediaTrackConstraints {
  return {
    width: {
      ideal: Math.round((profile.height * 16) / 9),
      max: Math.round((profile.height * 16) / 9),
    },
    height: { ideal: profile.height, max: profile.height },
    frameRate: { ideal: profile.fps, max: profile.fps },
  };
}

export function displayOptions(profile: VideoProfile) {
  return {
    video: videoConstraints(profile),
    audio: {
      echoCancellation: false,
      noiseSuppression: false,
      autoGainControl: false,
      channelCount: { ideal: 2 },
    },
    systemAudio: "include",
    windowAudio: "window",
  } satisfies DisplayMediaStreamOptions & {
    systemAudio: "include" | "exclude";
    windowAudio: "window";
  };
}

export function encodingBudget(
  profile: VideoProfile,
  hasAudio: boolean,
  networkBitrate: number | null,
) {
  const total =
    profile.mode === "auto" && networkBitrate !== null
      ? Math.min(profile.bitrate + (hasAudio ? 128_000 : 0), networkBitrate)
      : profile.bitrate + (hasAudio ? 128_000 : 0);
  const audio = hasAudio
    ? Math.min(128_000, Math.max(24_000, Math.round(total / 8)))
    : 0;
  return {
    video: Math.max(32_000, Math.min(profile.bitrate, total - audio)),
    audio,
  };
}

export async function configureSenders(
  pc: Pick<RTCPeerConnection, "getSenders">,
  profile: VideoProfile,
  networkBitrate: number | null = null,
  startup = false,
) {
  const budget = encodingBudget(
    profile,
    pc.getSenders().some((sender) => sender.track?.kind === "audio"),
    networkBitrate,
  );
  for (const sender of pc.getSenders()) {
    if (!sender.track) continue;
    const parameters = sender.getParameters();
    if (!parameters.encodings.length) throw new Error("浏览器未提供编码参数");
    for (const encoding of parameters.encodings) {
      encoding.maxBitrate =
        sender.track.kind === "audio" ? budget.audio : budget.video;
      if (sender.track.kind === "video") {
        encoding.maxFramerate = profile.fps;
        const { width = 1, height = 1 } = sender.track.getSettings();
        encoding.scaleResolutionDownBy = Math.max(
          1,
          width / Math.round((profile.height * 16) / 9),
          height / profile.height,
        );
      }
    }
    if (sender.track.kind === "video") {
      sender.track.contentHint = profile.mode === "auto" ? "detail" : "text";
      parameters.degradationPreference =
        profile.mode === "auto" && !startup
          ? "balanced"
          : "maintain-resolution";
    }
    await sender.setParameters(parameters);
  }
}
