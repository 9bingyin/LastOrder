export type VideoSample = {
  id: string;
  timestamp: number;
  bytes: number | null;
  width: number | null;
  height: number | null;
  dropped: number | null;
};

export type ViewerStats = {
  bitrate: number | null;
  resolution: string | null;
  dropped: number | null;
};

function numberField(
  record: Record<string, unknown>,
  key: string,
): number | null {
  const value = record[key];
  return typeof value === "number" && Number.isFinite(value) && value >= 0
    ? value
    : null;
}

export function videoSample(
  report: Pick<RTCStatsReport, "forEach">,
): VideoSample | null {
  let sample: VideoSample | null = null;
  report.forEach((entry: unknown) => {
    if (
      typeof entry !== "object" ||
      entry === null ||
      !("type" in entry) ||
      entry.type !== "inbound-rtp" ||
      !("kind" in entry) ||
      entry.kind !== "video"
    )
      return;
    const record: Record<string, unknown> = Object.fromEntries(
      Object.entries(entry),
    );
    const timestamp = numberField(record, "timestamp");
    if (typeof record.id !== "string" || timestamp === null) return;
    const current = {
      id: record.id,
      timestamp,
      bytes: numberField(record, "bytesReceived"),
      width: numberField(record, "frameWidth"),
      height: numberField(record, "frameHeight"),
      dropped: numberField(record, "framesDropped"),
    };
    if (!sample || (current.bytes ?? 0) > (sample.bytes ?? 0)) sample = current;
  });
  return sample;
}

export function viewerStats(
  current: VideoSample | null,
  previous: VideoSample | null,
  nativeWidth: number,
  nativeHeight: number,
): ViewerStats {
  let width = nativeWidth;
  let height = nativeHeight;
  if (current?.width && current.height) {
    width = current.width;
    height = current.height;
  }
  let bitrate: number | null = null;
  if (
    current &&
    previous &&
    current.id === previous.id &&
    current.timestamp > previous.timestamp &&
    current.bytes !== null &&
    previous.bytes !== null &&
    current.bytes >= previous.bytes
  ) {
    bitrate =
      ((current.bytes - previous.bytes) * 8000) /
      (current.timestamp - previous.timestamp);
  }
  return {
    bitrate: bitrate !== null && Number.isFinite(bitrate) ? bitrate : null,
    resolution: width > 0 && height > 0 ? `${width}×${height}` : null,
    dropped: current?.dropped ?? null,
  };
}
