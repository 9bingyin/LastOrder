import { configureSenders, videoConstraints } from "./quality";
import type { VideoProfile } from "./quality";

export class CaptureQuality {
  private profile: VideoProfile;
  private budget: number | null = null;
  private startup = true;
  private disposed = false;
  private pending = false;
  private revision = 0;
  private queue: Promise<void> = Promise.resolve();
  private timer: ReturnType<typeof setTimeout> | undefined;
  private readonly warmupDeadline = performance.now() + 2000;

  constructor(
    private readonly pc: RTCPeerConnection,
    private readonly stream: MediaStream,
    profile: VideoProfile,
    private readonly onError: (error: unknown) => void,
  ) {
    this.profile = profile;
    this.timer = setTimeout(() => void this.warmup(), 100);
  }

  dispose() {
    this.disposed = true;
    clearTimeout(this.timer);
  }

  setBudget(budget: number | null) {
    if (this.budget === budget) return;
    this.budget = budget;
    this.revision++;
    if (this.profile.mode === "auto") this.schedule();
  }

  applyProfile(profile: VideoProfile): Promise<void> {
    return this.enqueue(async () => {
      if (this.disposed) return;
      const track = this.stream.getVideoTracks()[0];
      if (!track) throw new Error("没有可分享的视频轨道");
      await track.applyConstraints(videoConstraints(profile));
      if (this.disposed) return;
      this.profile = profile;
      this.revision++;
      await this.configure();
    });
  }

  private enqueue(action: () => Promise<void>): Promise<void> {
    const next = this.queue.then(action);
    this.queue = next.catch(() => {});
    return next;
  }

  private configure() {
    if (this.disposed) return Promise.resolve();
    return configureSenders(this.pc, this.profile, this.budget, this.startup);
  }

  private schedule() {
    if (this.disposed || this.pending) return;
    this.pending = true;
    let applied = this.revision;
    void this.enqueue(async () => {
      applied = this.revision;
      await this.configure();
    })
      .catch((error: unknown) => {
        if (!this.disposed) this.onError(error);
      })
      .finally(() => {
        this.pending = false;
        if (!this.disposed && applied !== this.revision) this.schedule();
      });
  }

  private async warmup() {
    if (this.disposed) return;
    let frames = 0;
    try {
      const stats = await this.pc.getStats();
      stats.forEach((entry: unknown) => {
        if (
          typeof entry === "object" &&
          entry !== null &&
          "type" in entry &&
          entry.type === "outbound-rtp" &&
          "kind" in entry &&
          entry.kind === "video" &&
          "framesEncoded" in entry &&
          typeof entry.framesEncoded === "number"
        ) {
          frames += entry.framesEncoded;
        }
      });
    } catch {
      // 统计失败不能阻止编码器退出启动保护。
    }
    if (this.disposed) return;
    if (frames >= 5 || performance.now() >= this.warmupDeadline) {
      this.startup = false;
      this.revision++;
      if (this.profile.mode === "auto") this.schedule();
      return;
    }
    this.timer = setTimeout(() => void this.warmup(), 100);
  }
}
