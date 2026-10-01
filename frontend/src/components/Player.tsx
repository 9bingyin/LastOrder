import { Surface } from "@cloudflare/kumo/components/surface";
import { Button } from "@cloudflare/kumo/components/button";
import { Checkbox } from "@cloudflare/kumo/components/checkbox";
import { Dialog } from "@cloudflare/kumo/components/dialog";
import {
  GearIcon,
  MonitorIcon,
  PlayIcon,
  StopIcon,
} from "@phosphor-icons/react";
import type { SessionState } from "../session/useSession";

export function Player({ session }: { session: SessionState }) {
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
  if (!room) return null;
  return (
    <Surface className="overflow-hidden rounded-xl ring ring-kumo-line">
      <div className="relative flex aspect-video items-center justify-center bg-neutral-950">
        <video
          ref={video}
          autoPlay
          muted={!watchStream}
          playsInline
          controls
          className={`h-full w-full object-contain ${watchStream || captureStream ? "" : "hidden"}`}
          onLoadedData={reportReady}
        />
        {playbackBlocked && (
          <Button
            className="absolute"
            variant="primary"
            onClick={() => void perform(resumePlayback)}
          >
            播放
          </Button>
        )}
        {!watchStream && !captureStream && (
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
      </div>
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
    </Surface>
  );
}
