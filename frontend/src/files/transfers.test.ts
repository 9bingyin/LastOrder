import { afterEach, beforeEach, describe, expect, it, spyOn } from "bun:test";
import { chooseSaveFile, downloadFile, offerFile } from "./transfers";
import type { SharedFile } from "../api";

const auth = { token: "test-token", clientId: "page", roomId: "room" };
const file: SharedFile = {
  id: "file",
  name: "测试.bin",
  size: 4,
  publisherId: "peer",
  recipientId: "receiver",
  roomId: "room",
  state: "ready",
  blobTicket: "blob",
};
let fetchSpy: import("bun:test").Mock<typeof fetch>;
let originalWindow: PropertyDescriptor | undefined;
let written: number[];
let committed: boolean;
let discarded: boolean;

function output(onWrite?: () => void): WritableStream<Uint8Array> {
  return new WritableStream<Uint8Array>({
    write(chunk) {
      written.push(...chunk);
      onWrite?.();
    },
    close() {
      committed = true;
    },
    abort() {
      discarded = true;
    },
  });
}

function picker(stream: WritableStream<Uint8Array>) {
  Object.defineProperty(globalThis, "window", {
    configurable: true,
    value: {
      showSaveFilePicker: async () => ({ createWritable: async () => stream }),
    },
  });
}

beforeEach(() => {
  originalWindow = Object.getOwnPropertyDescriptor(globalThis, "window");
  fetchSpy = spyOn(globalThis, "fetch");
  written = [];
  committed = false;
  discarded = false;
  picker(output());
});
afterEach(() => {
  fetchSpy.mockRestore();
  if (originalWindow)
    Object.defineProperty(globalThis, "window", originalWindow);
  else Reflect.deleteProperty(globalThis, "window");
});

describe("文件保存", () => {
  it("大文件邀请只发送元数据，不提前上传文件内容", async () => {
    const offered = {
      ...file,
      size: 2 ** 31,
      state: "offered",
      blobTicket: null,
    };
    fetchSpy.mockResolvedValueOnce(Response.json(offered));
    const source = new File([new Uint8Array([1, 2])], "测试.bin");
    Object.defineProperty(source, "size", { value: 2 ** 31 });
    await offerFile(auth, "receiver", source, new AbortController().signal);
    const [url, options] = fetchSpy.mock.calls[0] ?? [];
    expect(url).toBe("/api/v1/files");
    expect(options?.method).toBe("POST");
    expect(JSON.parse(String(options?.body))).toEqual({
      roomId: "room",
      recipientId: "receiver",
      name: "测试.bin",
      size: 2 ** 31,
    });
  });
  it("带房间和页面认证获取文件，流式保存完整字节后才提交", async () => {
    fetchSpy.mockResolvedValueOnce(
      new Response(
        new ReadableStream<Uint8Array>({
          start(controller) {
            controller.enqueue(new Uint8Array([0, 255]));
            controller.enqueue(new Uint8Array([1, 128]));
            controller.close();
          },
        }),
        { headers: { "Content-Length": "4" } },
      ),
    );
    await downloadFile(
      auth,
      file,
      new AbortController().signal,
      () => {},
      await chooseSaveFile(file),
    );
    const [url, options] = fetchSpy.mock.calls[0] ?? [];
    expect(url).toBe("/api/v1/files/file?roomId=room");
    expect(options?.headers).toMatchObject({
      Authorization: "Bearer test-token",
      "X-Client-Id": "page",
    });
    expect(written).toEqual([0, 255, 1, 128]);
    expect(committed).toBe(true);
    expect(discarded).toBe(false);
  });

  it("截断和多余内容都不能提交为成功文件", async () => {
    for (const size of [2, 4]) {
      committed = false;
      discarded = false;
      picker(output());
      fetchSpy.mockResolvedValueOnce(
        new Response(new Uint8Array([1, 2, 3]), {
          headers: { "Content-Length": String(size) },
        }),
      );
      await expect(
        downloadFile(
          auth,
          { ...file, size },
          new AbortController().signal,
          () => {},
          await chooseSaveFile(file),
        ),
      ).rejects.toThrow();
      expect(committed).toBe(false);
      expect(discarded).toBe(true);
    }
  });

  it("取消保存会中断读取并丢弃尚未提交的文件", async () => {
    const cancellation = new AbortController();
    let sourceCancelled = false;
    picker(output(() => cancellation.abort()));
    fetchSpy.mockResolvedValueOnce(
      new Response(
        new ReadableStream<Uint8Array>({
          start(controller) {
            controller.enqueue(new Uint8Array([1, 2]));
          },
          cancel() {
            sourceCancelled = true;
          },
        }),
        { headers: { "Content-Length": "4" } },
      ),
    );
    await expect(
      downloadFile(
        auth,
        file,
        cancellation.signal,
        () => {},
        await chooseSaveFile(file),
      ),
    ).rejects.toThrow();
    expect(sourceCancelled).toBe(true);
    expect(discarded).toBe(true);
    expect(committed).toBe(false);
  });
});
