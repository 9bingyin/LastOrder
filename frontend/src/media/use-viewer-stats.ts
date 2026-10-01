import { useEffect } from "react";
import type { RefObject } from "react";
import type { MediaSession } from "../session/media-session";
import { videoSample, viewerStats } from "./viewer-stats";
import type { VideoSample, ViewerStats } from "./viewer-stats";

export function useViewerStats(
  watchStream: MediaStream | null,
  media: MediaSession,
  video: RefObject<HTMLVideoElement | null>,
  setReceiveStats: (stats: ViewerStats | null) => void,
) {
  useEffect(() => {
    const resource = media.watching;
    if (!watchStream || !resource) return;
    let disposed = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let previous: VideoSample | null = null;
    const valid = () => !disposed && media.watching === resource;
    const poll = async () => {
      try {
        if (resource.pc) {
          const report = await resource.pc.getStats();
          if (!valid()) return;
          const current = videoSample(report);
          setReceiveStats(
            viewerStats(
              current,
              previous,
              video.current?.videoWidth ?? 0,
              video.current?.videoHeight ?? 0,
            ),
          );
          previous = current;
        }
      } catch {
        if (valid()) {
          previous = null;
          setReceiveStats(null);
        }
      } finally {
        if (valid()) timer = setTimeout(() => void poll(), 1000);
      }
    };
    void poll();
    return () => {
      disposed = true;
      clearTimeout(timer);
    };
  }, [watchStream, media, video, setReceiveStats]);
}
