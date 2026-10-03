import { errorMessage, sharedFileSchema } from "../api";
import type { SharedFile } from "../api";

export type FileCredentials = {
  token: string;
  clientId: string;
  roomId: string;
};
export type SaveTarget = {
  createWritable(): Promise<WritableStream<Uint8Array>>;
};
declare global {
  interface Window {
    showSaveFilePicker?: (options: {
      suggestedName: string;
    }) => Promise<SaveTarget>;
  }
}
function headers(auth: FileCredentials): HeadersInit {
  return {
    Authorization: `Bearer ${auth.token}`,
    "X-Client-Id": auth.clientId,
  };
}
async function requireSuccess(response: Response): Promise<void> {
  if (response.ok) return;
  const data: unknown = await response.json().catch(() => null);
  if (
    data &&
    typeof data === "object" &&
    "error" in data &&
    data.error &&
    typeof data.error === "object" &&
    "message" in data.error &&
    typeof data.error.message === "string"
  )
    throw new Error(data.error.message);
  throw new Error(`文件操作失败（${response.status}）`);
}
export async function offerFile(
  auth: FileCredentials,
  recipientId: string,
  file: File,
  signal: AbortSignal,
): Promise<SharedFile> {
  const response = await fetch("/api/v1/files", {
    method: "POST",
    headers: { ...headers(auth), "Content-Type": "application/json" },
    body: JSON.stringify({
      roomId: auth.roomId,
      recipientId,
      name: file.name,
      size: file.size,
    }),
    signal,
  });
  await requireSuccess(response);
  return sharedFileSchema.parse(await response.json());
}
export async function changeFile(
  auth: FileCredentials,
  id: string,
  state: "accepted" | "rejected" | "cancelled" | "completed",
  signal: AbortSignal,
): Promise<void> {
  const response = await fetch(`/api/v1/files/${encodeURIComponent(id)}`, {
    method: "POST",
    headers: { ...headers(auth), "Content-Type": "application/json" },
    body: JSON.stringify({ roomId: auth.roomId, state }),
    signal,
  });
  await requireSuccess(response);
}
export async function uploadFile(
  auth: FileCredentials,
  id: string,
  file: File,
  signal: AbortSignal,
): Promise<void> {
  const query = new URLSearchParams({ roomId: auth.roomId });
  const response = await fetch(
    `/api/v1/files/${encodeURIComponent(id)}/content?${query}`,
    {
      method: "POST",
      headers: headers(auth),
      body: file,
      signal,
    },
  );
  await requireSuccess(response);
}
export async function chooseSaveFile(
  file: SharedFile,
): Promise<SaveTarget | null> {
  return (
    (await window.showSaveFilePicker?.({ suggestedName: file.name })) ?? null
  );
}
export async function downloadFile(
  auth: FileCredentials,
  file: SharedFile,
  signal: AbortSignal,
  saving: () => void,
  target: SaveTarget | null,
): Promise<void> {
  signal.throwIfAborted();
  const query = new URLSearchParams({ roomId: auth.roomId });
  const response = await fetch(
    `/api/v1/files/${encodeURIComponent(file.id)}?${query}`,
    { headers: headers(auth), signal },
  );
  await requireSuccess(response);
  const length = response.headers.get("Content-Length");
  if (length === null || Number(length) !== file.size) {
    await response.body?.cancel();
    throw new Error("文件大小与邀请信息不一致");
  }
  saving();
  if (target) {
    const output = await target.createWritable();
    if (!response.body) {
      await output.abort();
      throw new Error("文件响应缺少内容");
    }
    let received = 0;
    const verifyLength = new TransformStream<Uint8Array, Uint8Array>({
      transform(chunk, controller) {
        received += chunk.byteLength;
        if (received > file.size) throw new Error("文件内容超过预期大小");
        controller.enqueue(chunk);
      },
      flush() {
        if (received !== file.size) throw new Error("文件下载不完整，请重试");
      },
    });
    await response.body.pipeThrough(verifyLength).pipeTo(output, { signal });
  } else {
    const blob = await response.blob();
    signal.throwIfAborted();
    if (blob.size !== file.size) throw new Error("文件下载不完整，请重试");
    const url = URL.createObjectURL(blob);
    const anchor = document.createElement("a");
    anchor.href = url;
    anchor.download = file.name;
    document.body.append(anchor);
    anchor.click();
    anchor.remove();
    window.setTimeout(() => URL.revokeObjectURL(url), 1000);
  }
}
export function transferError(error: unknown): string | null {
  return error instanceof DOMException && error.name === "AbortError"
    ? null
    : errorMessage(error);
}
