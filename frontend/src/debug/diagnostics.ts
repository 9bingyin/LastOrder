import { z } from "zod";

export const diagnosticsSchema = z.looseObject({
  version: z.string(),
  snapshot: z.record(z.string(), z.unknown()),
  endpointAddressRaw: z.string(),
  boundSockets: z.array(z.string()),
  connections: z.array(
    z.looseObject({
      remoteId: z.string(),
      statsRaw: z.string(),
      paths: z.array(z.looseObject({ statsRaw: z.string() })),
    }),
  ),
  localWebRtc: z.record(z.string(), z.unknown()),
  actor: z.record(z.string(), z.unknown()),
});
export type Diagnostics = z.infer<typeof diagnosticsSchema>;
export type DebugEvent = { time: string; type: string; data: unknown };
const events: DebugEvent[] = [];

export function redactDebug(
  value: unknown,
  secrets: readonly string[] = [],
): unknown {
  const sensitive =
    /^(token|authorization|password|credential|joinCode|joinTicket|blobTicket|capability)$/i;
  const known = new Set(
    secrets.filter((secret) => secret && secret !== "[redacted]"),
  );
  const collect = (node: unknown) => {
    if (!node || typeof node !== "object") return;
    for (const [key, entry] of Object.entries(node)) {
      if (
        sensitive.test(key) &&
        typeof entry === "string" &&
        entry &&
        entry !== "[redacted]"
      )
        known.add(entry);
      if (
        key === "room" &&
        entry &&
        typeof entry === "object" &&
        "id" in entry &&
        typeof entry.id === "string" &&
        entry.id !== "[redacted]"
      )
        known.add(entry.id);
      collect(entry);
    }
  };
  collect(value);
  const walk = (node: unknown): unknown => {
    if (typeof node === "string") {
      let text = node
        .replace(/(^a=ice-pwd:)[^\r\n]*/gm, "$1[redacted]")
        .replace(/Bearer\s+[^\s"<>]+/gi, "Bearer [redacted]")
        .replace(/([#?&]token=)[^&\s"<>]+/gi, "$1[redacted]");
      for (const secret of known) text = text.split(secret).join("[redacted]");
      return text;
    }
    if (Array.isArray(node)) return node.map(walk);
    if (node && typeof node === "object")
      return Object.fromEntries(
        Object.entries(node).map(([key, entry]: [string, unknown]) => [
          key,
          sensitive.test(key) && entry != null ? "[redacted]" : walk(entry),
        ]),
      );
    return node;
  };
  return walk(value);
}

export function recordDebug(
  type: string,
  data: unknown,
  secrets: readonly string[] = [],
) {
  events.push({
    time: new Date().toISOString(),
    type,
    data: redactDebug(data, secrets),
  });
  if (events.length > 100) events.splice(0, events.length - 100);
}
export function debugEvents(): readonly DebugEvent[] {
  return events.slice();
}
export function clearDebugEvents() {
  events.length = 0;
}

export type DebugMedia = {
  kind: "capture" | "watch";
  id: string;
  pc: RTCPeerConnection | null;
  stream: MediaStream | null;
};

export async function mediaDiagnostics(media: DebugMedia) {
  const pc = media.pc;
  const stats: unknown[] = [];
  let statsError: string | null = null;
  try {
    (await pc?.getStats())?.forEach((entry: unknown) => stats.push(entry));
  } catch (error) {
    statsError = error instanceof Error ? error.message : String(error);
  }
  return {
    kind: media.kind,
    id: media.id,
    connection: pc
      ? {
          connectionState: pc.connectionState,
          iceConnectionState: pc.iceConnectionState,
          iceGatheringState: pc.iceGatheringState,
          signalingState: pc.signalingState,
          configuration: pc.getConfiguration(),
          localDescription: pc.localDescription?.toJSON(),
          remoteDescription: pc.remoteDescription?.toJSON(),
          senders: pc.getSenders().map((sender) => ({
            trackId: sender.track?.id,
            parameters: sender.getParameters(),
          })),
          receivers: pc.getReceivers().map((receiver) => ({
            trackId: receiver.track.id,
            parameters: receiver.getParameters(),
          })),
          transceivers: pc.getTransceivers().map((transceiver) => ({
            mid: transceiver.mid,
            direction: transceiver.direction,
            currentDirection: transceiver.currentDirection,
          })),
        }
      : null,
    stream: media.stream
      ? {
          id: media.stream.id,
          active: media.stream.active,
          tracks: media.stream.getTracks().map((track) => ({
            id: track.id,
            kind: track.kind,
            label: track.label,
            enabled: track.enabled,
            muted: track.muted,
            readyState: track.readyState,
            contentHint: track.contentHint,
            settings: track.getSettings(),
            constraints: track.getConstraints(),
            capabilities:
              typeof track.getCapabilities === "function"
                ? track.getCapabilities()
                : null,
          })),
        }
      : null,
    stats,
    statsError,
  };
}
