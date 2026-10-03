import { z } from "zod";
import { videoProfileSchema } from "./media/quality";
import { recordDebug } from "./debug/diagnostics";

const memberSchema = z.object({ id: z.string(), name: z.string() });
export const sharedFileSchema = z.object({
  id: z.string(),
  roomId: z.string(),
  name: z.string(),
  size: z.number().nonnegative(),
  publisherId: z.string(),
  recipientId: z.string(),
  state: z.enum([
    "offered",
    "accepted",
    "ready",
    "completed",
    "rejected",
    "cancelled",
  ]),
  blobTicket: z.string().nullable(),
});
export type SharedFile = z.infer<typeof sharedFileSchema>;
const shareSchema = z.object({
  profile: videoProfileSchema,
  audio: z.boolean(),
  id: z.string(),
  generation: z.string(),
  publisherId: z.string(),
  state: z.enum(["preparing", "live"]),
});
export const localMediaSchema = z.object({
  audio: z.boolean(),
  id: z.string(),
  shareId: z.string(),
  generation: z.string(),
  rtcPeerId: z.string(),
  clientId: z.string(),
  state: z.string(),
});
const snapshotSchema = z.object({
  eventSeq: z.number(),
  endpointId: z.string(),
  room: z
    .object({
      id: z.string(),
      ownerId: z.string(),
      revision: z.number(),
      members: z.record(z.string(), memberSchema),
      share: shareSchema.nullable(),
    })
    .nullable(),
  files: z.record(z.string(), sharedFileSchema).default({}),
  fileClients: z.record(z.string(), z.string()).default({}),
  joinCode: z.string().nullable(),
  joinTicket: z.string().nullable().default(null),
  capture: localMediaSchema.nullable(),
  subscription: localMediaSchema.nullable(),
  connection: z
    .object({
      path: z.string(),
      rttMs: z.number(),
      sentBytes: z.number(),
      receivedBytes: z.number(),
    })
    .nullable(),
  networkBitrate: z.number().int().min(64_000).max(64_000_000).nullable(),
  error: z.string().nullable(),
});
export const eventSchema = z.object({
  type: z.literal("snapshot"),
  clientId: z.string(),
  data: snapshotSchema,
});
export type Snapshot = z.infer<typeof snapshotSchema>;
export type LocalMedia = z.infer<typeof localMediaSchema>;
const errorSchema = z.object({ error: z.object({ message: z.string() }) });
export const emptySchema = z.object({});

export function readToken(): string | null {
  const fragment = new URLSearchParams(window.location.hash.slice(1));
  const token = fragment.get("token");
  if (token) {
    window.sessionStorage.setItem("lastorder.token", token);
    window.history.replaceState(
      null,
      "",
      window.location.pathname + window.location.search,
    );
  }
  return token ?? window.sessionStorage.getItem("lastorder.token");
}

export async function request<T>(
  token: string,
  clientId: string,
  path: string,
  schema: z.ZodType<T>,
  method = "POST",
  body?: unknown,
): Promise<T> {
  const started = performance.now();
  const secrets = [token];
  if (
    body &&
    typeof body === "object" &&
    "joinCode" in body &&
    typeof body.joinCode === "string"
  ) {
    secrets.push(body.joinCode, body.joinCode.toLowerCase());
  }
  recordDebug("api.request", { path, method, body }, secrets);
  const response = await fetch(`/api/v1${path}`, {
    method,
    headers: {
      Authorization: `Bearer ${token}`,
      "X-Client-Id": clientId,
      "Content-Type": "application/json",
      "Idempotency-Key": crypto.randomUUID(),
    },
    ...(body === undefined ? {} : { body: JSON.stringify(body) }),
    signal: AbortSignal.timeout(35_000),
  });
  const text = await response.text();
  let data: unknown;
  try {
    data = JSON.parse(text);
  } catch {
    recordDebug(
      "api.invalidResponse",
      { path, status: response.status, text: text.slice(0, 200) },
      secrets,
    );
    throw new Error(
      response.ok
        ? "本地服务返回了无效响应"
        : `请求失败（${response.status}）：${text.slice(0, 200)}`,
    );
  }
  recordDebug(
    "api.response",
    {
      path,
      status: response.status,
      durationMs: performance.now() - started,
      data,
    },
    secrets,
  );
  if (!response.ok) {
    const error = errorSchema.safeParse(data);
    throw new Error(
      error.success
        ? error.data.error.message
        : `请求失败（${response.status}）`,
    );
  }
  return schema.parse(data);
}

export function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : "操作失败，请重试";
}
