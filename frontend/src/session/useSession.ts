import { useCallback, useEffect, useRef, useState } from "react";
import {
  emptySchema,
  errorMessage,
  eventSchema,
  readToken,
  request,
} from "../api";
import type { Snapshot } from "../api";
import { generateNickname } from "../room/nickname";
import { recordDebug } from "../debug/diagnostics";
import type { DebugMedia } from "../debug/diagnostics";
import { usePageRoute } from "./use-page-route";
import { MediaSession } from "./media-session";
import { useViewerStats } from "../media/use-viewer-stats";
import type { ViewerStats } from "../media/viewer-stats";
import type { VideoProfile } from "../media/quality";
import { defaultProfile, videoProfileSchema } from "../media/quality";

export function useSession() {
  const [token] = useState(readToken);
  const [page, setPage] = useState<"/" | "/files" | "/debug">(() => {
    const path = window.location.pathname;
    return path === "/files" || path === "/debug" ? path : "/";
  });
  const debug = page === "/debug";
  const rawSnapshot = useRef<unknown>(null);
  const runtime = useRef<unknown>(null);
  const [snapshot, setSnapshot] = useState<Snapshot | null>(null);
  const [clientId, setClientId] = useState("");
  const [connected, setConnected] = useState(false);
  const [name, setName] = useState(() => generateNickname());
  const [joinCode, setJoinCode] = useState("");
  const [busy, setBusy] = useState(false);
  const [mode, setMode] = useState<VideoProfile["mode"]>(defaultProfile.mode);
  const [height, setHeight] = useState(String(defaultProfile.height));
  const [fps, setFps] = useState(String(defaultProfile.fps));
  const [bitrate, setBitrate] = useState(
    String(defaultProfile.bitrate / 1_000_000),
  );
  const [qualityOpen, setQualityOpen] = useState(false);
  const [qualityError, setQualityError] = useState<string | null>(null);
  const [receiveStats, setReceiveStats] = useState<ViewerStats | null>(null);
  const appliedProfile = useRef<VideoProfile>(defaultProfile);
  const [playbackBlocked, setPlaybackBlocked] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const [captureStream, setCaptureStream] = useState<MediaStream | null>(null);
  const [watchStream, setWatchStream] = useState<MediaStream | null>(null);
  const video = useRef<HTMLVideoElement>(null);
  const reportedReady = useRef<string | null>(null);
  const mounted = useRef(true);
  const actionEpoch = useRef(0);

  const [media] = useState(
    () =>
      new MediaSession({
        capture: (stream) => {
          setCaptureStream(stream);
          if (!stream) {
            setQualityOpen(false);
            setQualityError(null);
          }
        },
        watch: (stream) => {
          setWatchStream(stream);
          if (!stream) {
            reportedReady.current = null;
            setReceiveStats(null);
          }
        },
        error: setNotice,
      }),
  );
  const clearCapture = useCallback(() => media.clearCapture(), [media]);
  const clearWatching = useCallback(() => media.clearWatching(), [media]);

  usePageRoute(token, setPage, setQualityOpen);

  const readDebugMedia = useCallback((): DebugMedia[] => {
    const resources: DebugMedia[] = [];
    if (media.capture)
      resources.push({
        kind: "capture",
        id: media.capture.media.id,
        pc: media.capture.pc,
        stream: media.capture.stream,
      });
    if (media.watching)
      resources.push({
        kind: "watch",
        id: media.watching.media.id,
        pc: media.watching.pc,
        stream: media.watching.stream,
      });
    return resources;
  }, [media]);
  const readDebugRuntime = useCallback(() => {
    const element = video.current;
    const quality =
      typeof element?.getVideoPlaybackQuality === "function"
        ? element.getVideoPlaybackQuality()
        : null;
    return {
      page: runtime.current,
      snapshot: rawSnapshot.current,
      appliedProfile: appliedProfile.current,
      pendingCapture: !!media.pendingCapture,
      player: video.current
        ? {
            width: video.current.videoWidth,
            height: video.current.videoHeight,
            readyState: video.current.readyState,
            networkState: video.current.networkState,
            currentTime: video.current.currentTime,
            paused: video.current.paused,
            muted: video.current.muted,
            volume: video.current.volume,
            playbackQuality: quality
              ? {
                  creationTime: quality.creationTime,
                  totalVideoFrames: quality.totalVideoFrames,
                  droppedVideoFrames: quality.droppedVideoFrames,
                  corruptedVideoFrames: quality.corruptedVideoFrames,
                }
              : null,
          }
        : null,
    };
  }, []);

  useEffect(() => {
    mounted.current = true;
    if (!token) return;
    let disposed = false;
    let socket: WebSocket | null = null;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let attempts = 0;
    const connect = () => {
      socket = new WebSocket(
        `${window.location.protocol === "https:" ? "wss" : "ws"}://${window.location.host}/api/v1/events`,
        ["lastorder", token],
      );
      recordDebug("websocket.connect", { attempt: attempts });
      socket.onopen = () => recordDebug("websocket.open", {});
      socket.onerror = () => recordDebug("websocket.error", {});
      socket.onmessage = (event) => {
        try {
          const parsed: unknown = JSON.parse(String(event.data));
          recordDebug("websocket.message", parsed, [token]);
          rawSnapshot.current = parsed;
          const message = eventSchema.parse(parsed);
          media.reconcile(message.data);
          setSnapshot(message.data);
          setClientId(message.clientId);
          setConnected(true);
          attempts = 0;
        } catch (error) {
          recordDebug(
            "websocket.invalidMessage",
            { payload: String(event.data), error: errorMessage(error) },
            [token],
          );
          setNotice("本地服务消息格式无效，请重启 CLI");
          socket?.close();
        }
      };
      socket.onclose = (event) => {
        recordDebug(
          "websocket.close",
          { code: event.code, reason: event.reason, wasClean: event.wasClean },
          [token],
        );
        if (disposed) return;
        setConnected(false);
        setClientId("");
        actionEpoch.current++;
        setBusy(false);
        clearCapture();
        clearWatching();
        timer = setTimeout(connect, Math.min(1000 * 2 ** attempts++, 10_000));
      };
    };
    connect();
    return () => {
      disposed = true;
      mounted.current = false;
      clearTimeout(timer);
      socket?.close();
      clearCapture();
      clearWatching();
    };
  }, [token, clearCapture, clearWatching, media]);

  useEffect(() => media.reconcile(snapshot), [media, snapshot]);

  useEffect(() => {
    const element = video.current;
    if (!element) return;
    const stream = watchStream ?? captureStream;
    element.srcObject = stream;
    element.muted = !watchStream;
    setPlaybackBlocked(false);
    if (stream)
      void element.play().catch(() => {
        if (mounted.current && element.srcObject === stream)
          setPlaybackBlocked(true);
      });
  }, [watchStream, captureStream]);

  useViewerStats(watchStream, media, video, setReceiveStats);

  function openQuality() {
    const profile = appliedProfile.current;
    setMode(profile.mode);
    setHeight(String(profile.height));
    setFps(String(profile.fps));
    setBitrate(String(profile.bitrate / 1_000_000));
    setQualityError(null);
    setQualityOpen(true);
  }

  function readProfile() {
    const parsed = videoProfileSchema.safeParse({
      mode,
      height: Number(height),
      fps: Number(fps),
      bitrate: Math.round(Number(bitrate) * 1_000_000),
    });
    if (!parsed.success)
      throw new Error("画质配置无效：帧率 15–60 fps，码率 0.5–30 Mbps");
    return parsed.data;
  }

  async function updateQuality() {
    if (!token || !clientId) throw new Error("尚未连接到本地 CLI");
    const profile = readProfile();
    if (
      await media.updateQuality(
        token,
        clientId,
        profile,
        appliedProfile.current,
      )
    ) {
      appliedProfile.current = profile;
      setQualityOpen(false);
    }
  }

  async function api(path: string, method = "POST", body?: unknown) {
    if (!token || !clientId) throw new Error("尚未连接到本地 CLI");
    return request(token, clientId, path, emptySchema, method, body);
  }

  async function perform(
    action: () => Promise<void>,
    onError: (message: string) => void = setNotice,
  ) {
    const epoch = ++actionEpoch.current;
    setBusy(true);
    setNotice(null);
    try {
      await action();
    } catch (error) {
      recordDebug(
        "operation.error",
        { message: errorMessage(error) },
        token ? [token] : [],
      );
      if (mounted.current && epoch === actionEpoch.current)
        onError(errorMessage(error));
    } finally {
      if (mounted.current && epoch === actionEpoch.current) setBusy(false);
    }
  }

  function credentials() {
    if (!token || !clientId) throw new Error("尚未连接到本地 CLI");
    return { token, clientId };
  }
  async function stopCapture() {
    const auth = credentials();
    await media.stopCapture(auth.token, auth.clientId);
  }
  async function stopWatching() {
    const auth = credentials();
    await media.stopWatching(auth.token, auth.clientId);
  }
  async function startCapture() {
    const auth = credentials();
    await media.startCapture(
      auth.token,
      auth.clientId,
      appliedProfile.current,
      () => {
        void perform(stopCapture);
      },
    );
  }
  async function startWatching() {
    const auth = credentials();
    const share = snapshot?.room?.share;
    if (!share) throw new Error("房间没有活动分享");
    await media.startWatching(auth.token, auth.clientId, share);
  }
  function reportReady() {
    const subscription = media.watching?.media;
    if (subscription && reportedReady.current !== subscription.id) {
      reportedReady.current = subscription.id;
      void api(`/subscriptions/${subscription.id}/ready`).catch(
        (error: unknown) => setNotice(errorMessage(error)),
      );
    }
  }
  async function resumePlayback() {
    await video.current?.play();
    setPlaybackBlocked(false);
  }

  const room = snapshot?.room;
  const isPublisher = room?.share?.publisherId === snapshot?.endpointId;
  const ownCapture = snapshot?.capture?.clientId === clientId;
  const ownSubscription = snapshot?.subscription?.clientId === clientId;
  const available = connected && !busy;
  const error = notice ?? snapshot?.error;
  runtime.current = {
    clientId,
    connected,
    busy,
    notice,
    captureEpoch: media.captureEpoch,
    watchEpoch: media.watchEpoch,
    actionEpoch: actionEpoch.current,
  };

  return {
    token,
    debug,
    page,
    setPage,
    snapshot,
    clientId,
    connected,
    name,
    setName,
    joinCode,
    setJoinCode,
    busy,
    mode,
    setMode,
    height,
    setHeight,
    fps,
    setFps,
    bitrate,
    setBitrate,
    qualityOpen,
    setQualityOpen,
    qualityError,
    setQualityError,
    receiveStats,
    playbackBlocked,
    notice,
    setNotice,
    captureStream,
    watchStream,
    media,
    video,
    readDebugMedia,
    readDebugRuntime,
    openQuality,
    updateQuality,
    api,
    perform,
    clearCapture,
    clearWatching,
    stopCapture,
    stopWatching,
    startCapture,
    startWatching,
    reportReady,
    resumePlayback,
    room,
    isPublisher,
    ownCapture,
    ownSubscription,
    available,
    error,
  };
}

export type SessionState = ReturnType<typeof useSession>;
