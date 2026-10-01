import { Surface } from "@cloudflare/kumo/components/surface";
import { Button } from "@cloudflare/kumo/components/button";
import { Checkbox } from "@cloudflare/kumo/components/checkbox";
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
    shareAudio,
    setShareAudio,
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
  return (
    <Surface
      className={
        webFullscreen
          ? "fixed inset-0 z-50 rounded-none bg-neutral-950 ring-0"
          : "overflow-hidden rounded-xl ring ring-kumo-line"
      }
    >
      <div
        ref={containerRef}
        onMouseMove={handleMouseMove}
        onMouseLeave={handleMouseLeave}
        className={
          webFullscreen
            ? "relative flex h-full w-full items-center justify-center bg-neutral-950"
            : "relative flex aspect-video items-center justify-center bg-neutral-950"
        }
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
            onClick={() => void perform(resumePlayback)}
          >
            播放
          </Button>
        )}
        {!hasStream && (
          <div className="grid justify-items-center gap-3 px-5 text-center text-neutral-400">
            <MonitorIcon size={36} />
            <p className="text-sm">
              {room.share?.state === "live"
                ? "分享已开始"
                : room.share
                  ? "分享者正在准备画面"
                  : "尚未分享"}
            </p>
          </div>
        )}

        {hasStream && (
          <>
            <div
              className={`pointer-events-none absolute inset-x-0 bottom-0 h-28 bg-linear-to-t from-black/75 via-black/25 to-transparent transition-opacity duration-300 ${
                showControls || isPaused ? "opacity-100" : "opacity-0"
              }`}
            />
            <div
              className={`absolute inset-x-3 bottom-3 z-20 flex items-center justify-between rounded-xl border border-white/10 bg-neutral-900/80 px-3 py-1.5 text-white shadow-2xl backdrop-blur-xl transition-all duration-300 sm:inset-x-5 sm:bottom-4 sm:px-4 ${
                showControls || isPaused
                  ? "translate-y-0 opacity-100"
                  : "pointer-events-none translate-y-2 opacity-0"
              }`}
            >
              <div className="flex items-center gap-1.5 sm:gap-2">
                <button
                  type="button"
                  title={isPaused ? "播放" : "暂停"}
                  aria-label={isPaused ? "播放" : "暂停"}
                  onClick={togglePlay}
                  className="flex h-8 w-8 cursor-pointer items-center justify-center rounded-lg text-white/80 transition-all duration-150 hover:scale-105 hover:bg-white/15 hover:text-white active:scale-95"
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
                    title={isMuted || volume === 0 ? "取消静音" : "静音"}
                    aria-label={isMuted || volume === 0 ? "取消静音" : "静音"}
                    onClick={toggleMute}
                    className="flex h-8 w-8 cursor-pointer items-center justify-center rounded-lg text-white/80 transition-all duration-150 hover:scale-105 hover:bg-white/15 hover:text-white active:scale-95"
                  >
                    {isMuted || volume === 0 ? (
                      <SpeakerSlashIcon size={18} />
                    ) : volume < 0.5 ? (
                      <SpeakerLowIcon size={18} />
                    ) : (
                      <SpeakerHighIcon size={18} />
                    )}
                  </button>
                  <div className="flex w-0 items-center overflow-hidden opacity-0 transition-all duration-200 ease-out group-hover/vol:w-20 group-hover/vol:opacity-100 group-focus-within/vol:w-20 group-focus-within/vol:opacity-100">
                    <input
                      type="range"
                      min={0}
                      max={1}
                      step={0.02}
                      value={isMuted ? 0 : volume}
                      onChange={handleVolumeChange}
                      aria-label="音量调节"
                      title={`音量: ${Math.round((isMuted ? 0 : volume) * 100)}%`}
                      className="h-1.5 w-18 cursor-pointer appearance-none rounded-full bg-white/25 accent-white transition-all hover:bg-white/40 focus:outline-none"
                    />
                  </div>
                </div>

                <div className="ml-1 flex select-none items-center gap-1.5 rounded-full border border-red-500/30 bg-red-500/10 px-2 py-0.5 text-[11px] font-medium tracking-wide text-red-400">
                  <span className="h-1.5 w-1.5 animate-pulse rounded-full bg-red-500" />
                  <span>实时</span>
                </div>
              </div>

              <div className="flex items-center gap-1">
                {pipSupported && (
                  <button
                    type="button"
                    title="画中画"
                    aria-label="画中画"
                    onClick={() => void togglePip()}
                    className="flex h-8 w-8 cursor-pointer items-center justify-center rounded-lg text-white/80 transition-all duration-150 hover:scale-105 hover:bg-white/15 hover:text-white active:scale-95"
                  >
                    <PictureInPictureIcon size={18} />
                  </button>
                )}

                <button
                  type="button"
                  title={webFullscreen ? "退出网页全屏 (Esc)" : "网页全屏"}
                  aria-label={webFullscreen ? "退出网页全屏" : "网页全屏"}
                  onClick={() => setWebFullscreen((prev) => !prev)}
                  className={`flex h-8 w-8 cursor-pointer items-center justify-center rounded-lg text-white/80 transition-all duration-150 hover:scale-105 hover:bg-white/15 hover:text-white active:scale-95 ${
                    webFullscreen
                      ? "bg-white/20 text-white shadow-sm ring-1 ring-white/30"
                      : ""
                  }`}
                >
                  <AppWindowIcon size={18} />
                </button>

                <button
                  type="button"
                  title={isFullscreen ? "退出全屏" : "全屏"}
                  aria-label={isFullscreen ? "退出全屏" : "全屏"}
                  onClick={() => void toggleSystemFullscreen()}
                  className={`flex h-8 w-8 cursor-pointer items-center justify-center rounded-lg text-white/80 transition-all duration-150 hover:scale-105 hover:bg-white/15 hover:text-white active:scale-95 ${
                    isFullscreen
                      ? "bg-white/20 text-white shadow-sm ring-1 ring-white/30"
                      : ""
                  }`}
                >
                  {isFullscreen ? (
                    <CornersInIcon size={18} />
                  ) : (
                    <CornersOutIcon size={18} />
                  )}
                </button>
              </div>
            </div>
          </>
        )}
      </div>

      {!webFullscreen && (
        <div className="flex flex-wrap items-center justify-between gap-3 px-4 py-3">
          <span className="text-sm text-kumo-subtle">
            {watchStream ? "正在观看" : captureStream ? "本机预览" : "尚未播放"}
          </span>
          {watchStream && (
            <div
              role="group"
              aria-label="观看统计"
              className="flex flex-wrap items-center gap-x-3 gap-y-1 text-sm text-kumo-subtle tabular-nums"
            >
              <span title="实际接收视频码率">
                码率{" "}
                {receiveStats?.bitrate == null
                  ? "—"
                  : (receiveStats.bitrate / 1_000_000).toFixed(2)}{" "}
                Mbps
              </span>
              <span title="实际解码分辨率">
                分辨率 {receiveStats?.resolution ?? "—"}
              </span>
              <span title="本次观看累计解码丢帧，不等同于网络丢包">
                丢帧{" "}
                {receiveStats?.dropped == null
                  ? "—"
                  : Math.round(receiveStats.dropped)}
              </span>
            </div>
          )}
          <div className="flex flex-wrap items-center gap-2">
            {!room.share && !snapshot?.capture && (
              <Checkbox
                label="共享音频"
                checked={shareAudio}
                onCheckedChange={setShareAudio}
                disabled={!available}
              />
            )}
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
                variant="primary"
                icon={MonitorIcon}
                disabled={!available || !!room.share}
                onClick={() => void perform(startCapture)}
              >
                开始分享
              </Button>
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
                  icon={PlayIcon}
                  disabled={!available}
                  onClick={() => void perform(startWatching)}
                >
                  观看
                </Button>
              ))}
          </div>
        </div>
      )}
    </Surface>
  );
}
