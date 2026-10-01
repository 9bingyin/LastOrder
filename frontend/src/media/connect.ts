import { z } from "zod";
import { request } from "../api";
import type { LocalMedia } from "../api";
import { configureSenders, defaultProfile } from "./quality";
import type { VideoProfile } from "./quality";

const answerSchema = z.object({ type: z.literal("answer"), sdp: z.string() });

async function gather(pc: RTCPeerConnection): Promise<void> {
  if (pc.iceGatheringState === "complete") return;
  await new Promise<void>((resolve, reject) => {
    const cleanup = () => {
      clearTimeout(timer);
      pc.removeEventListener("icegatheringstatechange", changed);
    };
    const changed = () => {
      if (pc.iceGatheringState === "complete") {
        cleanup();
        resolve();
      }
    };
    const timer = setTimeout(() => {
      cleanup();
      reject(new Error("本机媒体地址收集超时"));
    }, 5000);
    pc.addEventListener("icegatheringstatechange", changed);
    changed();
  });
}

export async function connectMedia(
  token: string,
  clientId: string,
  media: LocalMedia,
  stream: MediaStream | null,
  onStream: (stream: MediaStream) => void,
  onFailed: () => void,
  profile: VideoProfile = defaultProfile,
): Promise<RTCPeerConnection> {
  const pc = new RTCPeerConnection({
    iceServers: [],
    bundlePolicy: "max-bundle",
  });
  try {
    if (stream) {
      const track = stream.getVideoTracks()[0];
      if (!track) throw new Error("没有可分享的视频轨道");
      track.contentHint = "text";
      for (const source of stream.getTracks()) {
        if (source.kind === "audio") source.contentHint = "music";
        pc.addTrack(source, stream);
      }
    } else {
      pc.addTransceiver("video", { direction: "recvonly" });
      if (media.audio) pc.addTransceiver("audio", { direction: "recvonly" });
      const remote = new MediaStream();
      pc.ontrack = (event) => {
        remote.addTrack(event.track);
        if (event.track.kind === "video") onStream(remote);
      };
    }
    for (const transceiver of pc.getTransceivers()) {
      const kind =
        transceiver.sender.track?.kind ?? transceiver.receiver.track.kind;
      const mime = kind === "audio" ? "audio/opus" : "video/vp8";
      const codecs = RTCRtpSender.getCapabilities(kind)?.codecs.filter(
        (codec) => codec.mimeType.toLowerCase() === mime,
      );
      if (!codecs?.length)
        throw new Error(
          "当前浏览器不支持 VP8 / Opus，请使用新版 Chrome、Edge 或 Firefox",
        );
      transceiver.setCodecPreferences(codecs);
    }
    pc.onconnectionstatechange = () => {
      if (pc.connectionState === "failed") onFailed();
    };
    await pc.setLocalDescription(await pc.createOffer());
    await gather(pc);
    const sdp = pc.localDescription?.sdp;
    if (!sdp) throw new Error("无法创建本机媒体连接");
    const answer = await request(
      token,
      clientId,
      `/rtc/${media.rtcPeerId}/offer`,
      answerSchema,
      "POST",
      { sdp },
    );
    await pc.setRemoteDescription(answer);
    if (stream) await configureSenders(pc, profile, null, true);
    return pc;
  } catch (error) {
    pc.close();
    throw error;
  }
}
