import { afterEach, beforeEach, describe, expect, it, spyOn } from "bun:test";
import { emptySchema, eventSchema, localMediaSchema, request } from "./api";
import {
  clearDebugEvents,
  debugEvents,
  recordDebug,
  redactDebug,
} from "./debug/diagnostics";

let fetchSpy: import("bun:test").Mock<
  (...args: Parameters<typeof fetch>) => ReturnType<typeof fetch>
>;
beforeEach(() => {
  fetchSpy = spyOn(globalThis, "fetch");
});
afterEach(() => {
  fetchSpy.mockRestore();
  clearDebugEvents();
});

const media = {
  id: "capture",
  shareId: "share",
  generation: "generation",
  rtcPeerId: "rtc",
  clientId: "page",
  audio: false,
  state: "preparing",
};

describe("本地 API 契约", () => {
  it("完整媒体响应能进入流程，缺少任何必要字段都会拒绝", async () => {
    fetchSpy.mockResolvedValueOnce(Response.json(media));
    expect(
      await request("test-token", "page", "/shares", localMediaSchema),
    ).toEqual(media);
    for (const field of Object.keys(media)) {
      const incomplete: Record<string, unknown> = { ...media };
      delete incomplete[field];
      fetchSpy.mockResolvedValueOnce(Response.json(incomplete));
      await expect(
        request("test-token", "page", "/shares", localMediaSchema),
      ).rejects.toThrow();
    }
  });
  it("兼容没有 Ticket 的空房间快照", () => {
    const parsed = eventSchema.parse({
      type: "snapshot",
      clientId: "page",
      data: {
        eventSeq: 1,
        endpointId: "node",
        room: null,
        joinCode: null,
        capture: null,
        subscription: null,
        connection: null,
        networkBitrate: null,
        error: null,
      },
    });
    expect(parsed.data.joinTicket).toBeNull();
    expect(parsed.data.discoveryError).toBeNull();
  });
  it("保留发现服务失败状态和可用 Ticket", () => {
    const parsed = eventSchema.parse({
      type: "snapshot",
      clientId: "page",
      data: {
        eventSeq: 1,
        endpointId: "node",
        room: {
          id: "room",
          ownerId: "node",
          revision: 1,
          members: { node: { id: "node", name: "房主" } },
          share: null,
        },
        joinCode: "room",
        joinTicket: "lastorder-ticket",
        discoveryError: "Error sending http request: connection reset",
        capture: null,
        subscription: null,
        connection: null,
        networkBitrate: null,
        error: null,
      },
    });
    expect(parsed.data.joinTicket).toBe("lastorder-ticket");
    expect(parsed.data.discoveryError).toContain("connection reset");
    expect(parsed.data.error).toBeNull();
  });
  it("拒绝未知的 WebSocket 事件", () => {
    expect(eventSchema.safeParse({ type: "unknown" }).success).toBe(false);
  });
  it("退出后成功、失败与非 JSON 响应日志仍隐藏 UUID 和 Ticket", async () => {
    for (const joinCode of [
      "ABCDEF00-1234-4ABC-8123-ABCDEF012345",
      "LASTORDER-TEST-CREDENTIAL",
    ]) {
      for (const status of [200, 400, 502]) {
        clearDebugEvents();
        const text = `trace-marker test-token ${joinCode} ${joinCode.toLowerCase()}`;
        const response =
          status === 502
            ? new Response(text, { status })
            : Response.json(
                {
                  room: { id: "decoded-room-credential" },
                  message: text,
                  error: { message: text },
                },
                { status },
              );
        fetchSpy.mockResolvedValueOnce(response);
        const operation = request(
          "test-token",
          "page",
          "/room/join",
          emptySchema,
          "POST",
          { joinCode, displayName: "成员" },
        );
        if (status === 200) await operation;
        else
          await expect(operation).rejects.toThrow(
            status === 400 ? "trace-marker" : "502",
          );
        const call = fetchSpy.mock.calls.at(-1);
        expect(call?.[0]).toBe("/api/v1/room/join");
        expect(call?.[1]?.headers).toMatchObject({
          Authorization: "Bearer test-token",
          "X-Client-Id": "page",
        });
        recordDebug("websocket.message", {
          data: { joinCode: null, room: null },
        });
        const copied = JSON.stringify(
          redactDebug({ snapshot: { joinCode: null }, events: debugEvents() }),
        );
        expect(copied).toContain("trace-marker");
        for (const secret of [
          joinCode,
          joinCode.toLowerCase(),
          "test-token",
          "decoded-room-credential",
        ])
          expect(copied).not.toContain(secret);
      }
    }
  });
});
