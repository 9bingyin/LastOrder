import { Surface } from "@cloudflare/kumo/components/surface";
import { Button } from "@cloudflare/kumo/components/button";
import { Dialog } from "@cloudflare/kumo/components/dialog";
import {
  AppWindowIcon,
  CornersInIcon,
  CornersOutIcon,
  GearIcon,
  MonitorIcon,
  PauseIcon,
  PictureInPictureIcon,
  PlayIcon,
  SpeakerHighIcon,
  SpeakerLowIcon,
  SpeakerSlashIcon,
  StopIcon,
} from "@phosphor-icons/react";
import { useCallback, useEffect, useRef, useState } from "react";
import type { SessionState } from "../session/useSession";

const controlClass =
  "flex size-8 cursor-pointer items-center justify-center rounded-lg text-white/80 hover:bg-white/15 hover:text-white focus-visible:outline-2 focus-visible:outline-white/60";
const activeControlClass = "bg-white/20 text-white";

export function Player({ session }: { session: SessionState }) {
  const [webFullscreen, setWebFullscreen] = useState(false);
  const [isFullscreen, setIsFullscreen] = useState(false);
  const [isPaused, setIsPaused] = useState(false);
  const [isMuted, setIsMuted] = useState(true);
  const [volume, setVolume] = useState(1);
  const prevVolumeRef = useRef(1);
  const [showControls, setShowControls] = useState(true);
  const containerRef = useRef<HTMLDivElement>(null);
  const hideTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  const {
    room,
    snapshot,
    video,
    watchStream,
    captureStream,
    reportReady,
    playbackBlocked,
    perform,
    resumePlayback,
    receiveStats,
    available,
    ownCapture,
    media,
    stopCapture,
    startCapture,
    isPublisher,
    ownSubscription,
    stopWatching,
    startWatching,
  } = session;

  const hasStream = Boolean(watchStream || captureStream);

  useEffect(() => {
    if (!hasStream) {
      setWebFullscreen(false);
    }
  }, [hasStream]);

  useEffect(() => {
    if (!webFullscreen) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        setWebFullscreen(false);
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [webFullscreen]);

  useEffect(() => {
    if (!webFullscreen) return;
    const originalOverflow = document.body.style.overflow;
    document.body.style.overflow = "hidden";
    return () => {
      document.body.style.overflow = originalOverflow;
    };
  }, [webFullscreen]);

  useEffect(() => {
    const onFullscreenChange = () => {
      setIsFullscreen(document.fullscreenElement === containerRef.current);
    };
    document.addEventListener("fullscreenchange", onFullscreenChange);
    return () => document.removeEventListener("fullscreenchange", onFullscreenChange);
  }, []);

  const handleMouseMove = useCallback(() => {
    setShowControls(true);
    if (hideTimer.current) clearTimeout(hideTimer.current);
    hideTimer.current = setTimeout(() => {
      setShowControls(false);
    }, 2500);
  }, []);

  const handleMouseLeave = useCallback(() => {
    if (hideTimer.current) clearTimeout(hideTimer.current);
    setShowControls(false);
  }, []);

  const togglePlay = useCallback(() => {
    const element = video.current;
    if (!element) return;
    if (element.paused) {
      void perform(resumePlayback);
    } else {
      element.pause();
    }
  }, [perform, resumePlayback, video]);

  const handleVolumeChange = useCallback(
    (e: React.ChangeEvent<HTMLInputElement>) => {
      const val = parseFloat(e.target.value);
      setVolume(val);
      const element = video.current;
      if (!element) return;
      element.volume = val;
      if (val === 0) {
        element.muted = true;
        setIsMuted(true);
      } else if (element.muted) {
        element.muted = false;
        setIsMuted(false);
      }
    },
    [video],
  );

  const toggleMute = useCallback(() => {
    const element = video.current;
    if (!element) return;
    if (element.muted || element.volume === 0) {
      element.muted = false;
      setIsMuted(false);
      if (element.volume === 0) {
        const restore = prevVolumeRef.current > 0 ? prevVolumeRef.current : 1;
        element.volume = restore;
        setVolume(restore);
      }
    } else {
      prevVolumeRef.current = element.volume;
      element.muted = true;
      setIsMuted(true);
    }
  }, [video]);

  const toggleSystemFullscreen = useCallback(async () => {
    const container = containerRef.current;
    if (!container) return;
    try {
      if (!document.fullscreenElement) {
        await container.requestFullscreen();
      } else {
        await document.exitFullscreen();
      }
    } catch {
      // 忽略全屏切换异常
    }
  }, []);

  const togglePip = useCallback(async () => {
    const element = video.current;
    if (!element) return;
    try {
      if (document.pictureInPictureElement) {
        await document.exitPictureInPicture();
      } else {
        await element.requestPictureInPicture();
      }
    } catch {
      // 忽略画中画异常
    }
  }, [video]);

  const pipSupported =
    typeof document !== "undefined" && Boolean(document.pictureInPictureEnabled);

  if (!room) return null;
  const controlsVisible = showControls || isPaused;
  const silent = isMuted || volume === 0;
  const status = watchStream
    ? { label: "正在观看", dot: "bg-kumo-success" }
    : captureStream
      ? { label: "本机预览", dot: "bg-kumo-danger" }
      : { label: "尚未播放", dot: "bg-kumo-line" };
  const stats = watchStream
    ? [
        [
          "实际接收视频码率",
          `${receiveStats?.bitrate == null ? "—" : (receiveStats.bitrate / 1_000_000).toFixed(2)} Mbps`,
        ],
        ["实际解码分辨率", receiveStats?.resolution ?? "—"],
        [
          "本次观看累计解码丢帧，不等同于网络丢包",
          `丢帧 ${receiveStats?.dropped == null ? "—" : Math.round(receiveStats.dropped)}`,
        ],
      ]
    : [];
  return (
    <Surface
      className={
        webFullscreen
          ? "fixed inset-0 z-50 rounded-none bg-black ring-0"
          : "overflow-hidden rounded-xl ring ring-kumo-line"
      }
    >
      <div
        ref={containerRef}
        onMouseMove={handleMouseMove}
        onMouseLeave={handleMouseLeave}
        className={`relative flex items-center justify-center bg-black ${webFullscreen ? "h-full w-full" : "aspect-video"} ${hasStream && !controlsVisible ? "cursor-none" : ""}`}
      >
        <video
          ref={video}
          autoPlay
          muted={!watchStream}
          playsInline
          className={`h-full w-full object-contain ${hasStream ? "" : "hidden"}`}
          onLoadedData={reportReady}
          onPlay={() => setIsPaused(false)}
          onPause={() => setIsPaused(true)}
          onVolumeChange={() => {
            if (video.current) {
              setIsMuted(video.current.muted);
              setVolume(video.current.volume);
            }
          }}
          onDoubleClick={() => setWebFullscreen((prev) => !prev)}
        />
        {playbackBlocked && (
          <Button
            className="absolute z-10"
            variant="primary"
            icon={PlayIcon}
            onClick={() => void perform(resumePlayback)}
          >
            播放
          </Button>
        )}
        {!hasStream && (
          <div className="grid justify-items-center gap-3 px-5 text-center">
            <span className="flex size-14 items-center justify-center rounded-full bg-white/5 text-white/50 ring ring-white/10">
              <MonitorIcon size={28} aria-hidden="true" />
            </span>
            <p className="text-sm text-white/60">
              {room.share?.state === "live"
                ? "分享已开始"
                : room.share
                  ? "分享者正在准备画面"
                  : "尚未分享"}
            </p>
          </div>
        )}

        {hasStream && (
          <div
            className={`absolute inset-x-0 bottom-0 z-20 flex items-center justify-between gap-2 bg-linear-to-t from-black/80 to-transparent px-3 pt-10 pb-2 text-white transition-opacity duration-200 ${
              controlsVisible ? "opacity-100" : "pointer-events-none opacity-0"
            }`}
          >
            <div className="flex items-center gap-1">
              <button
                type="button"
                title={isPaused ? "播放" : "暂停"}
                aria-label={isPaused ? "播放" : "暂停"}
                onClick={togglePlay}
                className={controlClass}
              >
                {isPaused ? (
                  <PlayIcon size={18} weight="fill" />
                ) : (
                  <PauseIcon size={18} weight="fill" />
                )}
              </button>
              <div className="group/vol flex items-center">
                <button
                  type="button"
                  title={silent ? "取消静音" : "静音"}
                  aria-label={silent ? "取消静音" : "静音"}
                  onClick={toggleMute}
                  className={controlClass}
                >
                  {silent ? (
                    <SpeakerSlashIcon size={18} />
                  ) : volume < 0.5 ? (
                    <SpeakerLowIcon size={18} />
                  ) : (
                    <SpeakerHighIcon size={18} />
                  )}
                </button>
                <div className="flex w-0 items-center overflow-hidden transition-[width] duration-200 group-hover/vol:w-20 group-focus-within/vol:w-20">
                  <input
                    type="range"
                    min={0}
                    max={1}
                    step={0.02}
                    value={isMuted ? 0 : volume}
                    onChange={handleVolumeChange}
                    aria-label="音量调节"
                    title={`音量: ${Math.round((isMuted ? 0 : volume) * 100)}%`}
                    className="ml-1 h-1 w-18 cursor-pointer appearance-none rounded-full bg-white/30 accent-white focus:outline-none"
                  />
                </div>
              </div>
              <span className="ml-2 inline-flex select-none items-center gap-1.5 text-sm font-medium text-white/90">
                <span className="size-1.5 animate-pulse rounded-full bg-red-500" />
                实时
              </span>
            </div>

            <div className="flex items-center gap-1">
              {pipSupported && (
                <button
                  type="button"
                  title="画中画"
                  aria-label="画中画"
                  onClick={() => void togglePip()}
                  className={controlClass}
                >
                  <PictureInPictureIcon size={18} />
                </button>
              )}
              <button
                type="button"
                title={webFullscreen ? "退出网页全屏 (Esc)" : "网页全屏"}
                aria-label={webFullscreen ? "退出网页全屏" : "网页全屏"}
                onClick={() => setWebFullscreen((prev) => !prev)}
                className={`${controlClass} ${webFullscreen ? activeControlClass : ""}`}
              >
                <AppWindowIcon size={18} />
              </button>
              <button
                type="button"
                title={isFullscreen ? "退出全屏" : "全屏"}
                aria-label={isFullscreen ? "退出全屏" : "全屏"}
                onClick={() => void toggleSystemFullscreen()}
                className={`${controlClass} ${isFullscreen ? activeControlClass : ""}`}
              >
                {isFullscreen ? (
                  <CornersInIcon size={18} />
                ) : (
                  <CornersOutIcon size={18} />
                )}
              </button>
            </div>
          </div>
        )}
      </div>

      {!webFullscreen && (
        <div className="flex flex-wrap items-center justify-between gap-3 border-t border-kumo-line px-4 py-3">
          <div className="flex min-w-0 flex-wrap items-center gap-x-3 gap-y-1 text-sm text-kumo-subtle">
            <span className="inline-flex items-center gap-2 font-medium text-kumo-default">
              <span
                aria-hidden="true"
                className={`size-2 rounded-full ${status.dot}`}
              />
              {status.label}
            </span>
            {stats.length > 0 && (
              <span
                role="group"
                aria-label="观看统计"
                className="flex flex-wrap items-center gap-x-2 tabular-nums"
              >
                {stats.map(([title, value], index) => (
                  <span key={title} title={title}>
                    {index > 0 && (
                      <span aria-hidden="true" className="mr-2 text-kumo-line">
                        /
                      </span>
                    )}
                    {value}
                  </span>
                ))}
              </span>
            )}
          </div>
          <div className="flex flex-wrap items-center gap-2">
            {ownCapture && captureStream && (
              <Dialog.Trigger
                render={
                  <Button
                    icon={GearIcon}
                    disabled={!available || !media.capture?.quality}
                  >
                    画质
                  </Button>
                }
              />
            )}
            {!isPublisher &&
              room.share?.state === "live" &&
              (snapshot?.subscription ? (
                <Button
                  icon={StopIcon}
                  disabled={!available || !ownSubscription}
                  onClick={() => void perform(stopWatching)}
                >
                  停止观看
                </Button>
              ) : (
                <Button
                  variant="primary"
                  icon={PlayIcon}
                  disabled={!available}
                  onClick={() => void perform(startWatching)}
                >
                  观看
                </Button>
              ))}
            {snapshot?.capture ? (
              <Button
                variant="secondary-destructive"
                icon={StopIcon}
                disabled={!available || !ownCapture}
                onClick={() => void perform(stopCapture)}
              >
                停止分享
              </Button>
            ) : (
              <Button
                variant={room.share ? "secondary" : "primary"}
                icon={MonitorIcon}
                disabled={!available || !!room.share}
                onClick={() => void perform(startCapture)}
              >
                开始分享
              </Button>
            )}
          </div>
        </div>
      )}
    </Surface>
  );
}
