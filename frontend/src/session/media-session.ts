import { emptySchema, errorMessage, localMediaSchema, request } from "../api";
import type { LocalMedia, Snapshot } from "../api";
import { connectMedia } from "../media/connect";
import { CaptureQuality } from "../media/capture-quality";
import { displayOptions } from "../media/quality";
import type { VideoProfile } from "../media/quality";

export type MediaResource = {
  media: LocalMedia;
  pc: RTCPeerConnection | null;
  stream: MediaStream | null;
  observed: boolean;
};
type CaptureResource = MediaResource & { quality: CaptureQuality | null };
type Listeners = {
  capture: (stream: MediaStream | null) => void;
  watch: (stream: MediaStream | null) => void;
  error: (message: string) => void;
};

export class MediaSession {
  capture: CaptureResource | null = null;
  watching: MediaResource | null = null;
  pendingCapture: MediaStream | null = null;
  captureEpoch = 0;
  watchEpoch = 0;
  private snapshot: Snapshot | null = null;

  constructor(private readonly listeners: Listeners) {}

  clearCapture() {
    this.captureEpoch++;
    this.pendingCapture?.getTracks().forEach((track) => track.stop());
    this.pendingCapture = null;
    const resource = this.capture;
    this.capture = null;
    resource?.quality?.dispose();
    resource?.pc?.close();
    resource?.stream?.getTracks().forEach((track) => track.stop());
    this.listeners.capture(null);
  }

  clearWatching() {
    this.watchEpoch++;
    const resource = this.watching;
    this.watching = null;
    resource?.pc?.close();
    this.listeners.watch(null);
  }

  reconcile(snapshot: Snapshot | null) {
    this.snapshot = snapshot;
    const capture = this.capture;
    if (capture) {
      if (snapshot?.capture?.id === capture.media.id) {
        capture.observed = true;
        capture.quality?.setBudget(snapshot.networkBitrate);
      } else if (capture.observed) this.clearCapture();
    }
    const watching = this.watching;
    if (watching) {
      if (snapshot?.subscription?.id === watching.media.id)
        watching.observed = true;
      else if (watching.observed) this.clearWatching();
    }
  }

  async updateQuality(
    token: string,
    clientId: string,
    profile: VideoProfile,
    previous: VideoProfile,
  ) {
    const resource = this.capture;
    if (!resource?.quality) throw new Error("尚未开始分享");
    try {
      await resource.quality.applyProfile(profile);
      if (this.capture !== resource) return false;
      await request(
        token,
        clientId,
        `/shares/${resource.media.shareId}/quality`,
        emptySchema,
        "POST",
        profile,
      );
      return this.capture === resource;
    } catch (error) {
      if (this.capture === resource)
        await resource.quality.applyProfile(previous).catch(() => {});
      throw error;
    }
  }

  async stopCapture(token: string, clientId: string) {
    const id = this.capture?.media.shareId ?? this.snapshot?.capture?.shareId;
    this.clearCapture();
    if (id)
      await request(token, clientId, `/shares/${id}`, emptySchema, "DELETE");
  }

  async stopWatching(token: string, clientId: string) {
    const id = this.watching?.media.id ?? this.snapshot?.subscription?.id;
    this.clearWatching();
    if (id)
      await request(
        token,
        clientId,
        `/subscriptions/${id}`,
        emptySchema,
        "DELETE",
      );
  }

  async startCapture(
    token: string,
    clientId: string,
    profile: VideoProfile,
    onEnded: () => void,
  ) {
    if (!navigator.mediaDevices?.getDisplayMedia)
      throw new Error("当前浏览器不支持屏幕捕获");
    const epoch = ++this.captureEpoch;
    const valid = () => this.captureEpoch === epoch;
    const stream = await navigator.mediaDevices.getDisplayMedia(
      displayOptions(profile),
    );
    let resource: CaptureResource | null = null;
    const stopTracks = () =>
      stream.getTracks().forEach((track) => track.stop());
    const retire = async () => {
      resource?.quality?.dispose();
      resource?.pc?.close();
      stopTracks();
      if (resource && this.capture === resource) this.clearCapture();
      if (this.pendingCapture === stream) this.pendingCapture = null;
      if (resource)
        await request(
          token,
          clientId,
          `/shares/${resource.media.shareId}`,
          emptySchema,
          "DELETE",
        ).catch(() => {});
    };
    try {
      if (!valid()) {
        stopTracks();
        return;
      }
      this.pendingCapture = stream;
      const media = await request(
        token,
        clientId,
        "/shares",
        localMediaSchema,
        "POST",
        {
          profile,
          audio: stream.getAudioTracks().length > 0,
        },
      );
      resource = { media, pc: null, stream, observed: false, quality: null };
      if (
        !valid() ||
        !stream.getVideoTracks().some((track) => track.readyState === "live")
      ) {
        await retire();
        return;
      }
      this.pendingCapture = null;
      this.capture = resource;
      this.listeners.capture(stream);
      stream.getVideoTracks()[0]?.addEventListener(
        "ended",
        () => {
          if (this.capture === resource) onEnded();
        },
        { once: true },
      );
      const pc = await connectMedia(
        token,
        clientId,
        media,
        stream,
        () => {},
        () => {
          if (this.capture !== resource) return;
          this.listeners.error("本机捕获连接失败，请重新分享");
          void this.stopCapture(token, clientId).catch(() => {});
        },
        profile,
      );
      resource.pc = pc;
      if (!valid() || this.capture !== resource) await retire();
      else {
        const current = resource;
        current.quality = new CaptureQuality(pc, stream, profile, (error) => {
          if (this.capture === current)
            this.listeners.error(`自动画质调整失败：${errorMessage(error)}`);
        });
        if (this.snapshot?.capture?.id === media.id)
          current.quality.setBudget(this.snapshot.networkBitrate);
      }
    } catch (error) {
      const current = valid();
      await retire();
      if (current) {
        if (this.captureEpoch === epoch) this.clearCapture();
        throw error;
      }
    }
  }

  async startWatching(
    token: string,
    clientId: string,
    share: { id: string; generation: string },
  ) {
    const epoch = ++this.watchEpoch;
    const valid = () => this.watchEpoch === epoch;
    const media = await request(
      token,
      clientId,
      "/subscriptions",
      localMediaSchema,
      "POST",
      {
        shareId: share.id,
        generation: share.generation,
      },
    );
    const resource: MediaResource = {
      media,
      pc: null,
      stream: null,
      observed: false,
    };
    const retire = async () => {
      resource.pc?.close();
      if (this.watching === resource) this.clearWatching();
      await request(
        token,
        clientId,
        `/subscriptions/${media.id}`,
        emptySchema,
        "DELETE",
      ).catch(() => {});
    };
    if (!valid()) {
      await retire();
      return;
    }
    this.watching = resource;
    try {
      const pc = await connectMedia(
        token,
        clientId,
        media,
        null,
        (stream) => {
          if (this.watching === resource) this.listeners.watch(stream);
        },
        () => {
          if (this.watching !== resource) return;
          this.listeners.error("本机播放连接失败，请重新观看");
          void this.stopWatching(token, clientId).catch(() => {});
        },
      );
      resource.pc = pc;
      if (!valid() || this.watching !== resource) await retire();
    } catch (error) {
      const current = valid();
      await retire();
      if (current) throw error;
    }
  }
}
