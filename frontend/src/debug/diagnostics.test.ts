import { beforeEach, describe, expect, it } from "bun:test";
import {
  clearDebugEvents,
  debugEvents,
  diagnosticsSchema,
  recordDebug,
  redactDebug,
} from "./diagnostics";

beforeEach(clearDebugEvents);
describe("Debug 诊断", () => {
  it("保留原始字段和统计值，只脱敏凭证", () => {
    expect(
      redactDebug({
        joinCode: "secret-room",
        token: "private",
        bytes: 12,
        unknownField: { futureMetric: 99 },
        sdp: "v=0\r\na=ice-pwd:password\r\na=rtpmap:96 VP8/90000",
      }),
    ).toEqual({
      joinCode: "[redacted]",
      token: "[redacted]",
      bytes: 12,
      unknownField: { futureMetric: 99 },
      sdp: "v=0\r\na=ice-pwd:[redacted]\r\na=rtpmap:96 VP8/90000",
    });
  });
  it("加入码在 room.id 或其他字符串中重复出现时也脱敏", () => {
    expect(
      redactDebug({
        room: { id: "secret-room" },
        joinCode: "secret-room",
        message: "room secret-room",
      }),
    ).toEqual({
      room: { id: "[redacted]" },
      joinCode: "[redacted]",
      message: "room [redacted]",
    });
  });
  it("Ticket 及仅含 room.id 的加入响应都会脱敏", () => {
    expect(
      redactDebug({
        joinTicket: "lastorder-secret",
        room: { id: "room-secret" },
        message: "lastorder-secret room-secret",
      }),
    ).toEqual({
      joinTicket: "[redacted]",
      room: { id: "[redacted]" },
      message: "[redacted] [redacted]",
    });
    expect(redactDebug({ room: { id: "room-secret" } })).toEqual({
      room: { id: "[redacted]" },
    });
  });

  it("错误消息、URL 与嵌套数组中的认证令牌也脱敏", () => {
    for (const [message, secret] of [
      ["trace-marker Bearer bearer-secret", "bearer-secret"],
      ["trace-marker /#token=url-secret", "url-secret"],
      ["trace-marker private-token", "private-token"],
    ]) {
      const safe = JSON.stringify(
        redactDebug({ nested: [{ message }] }, ["private-token"]),
      );
      expect(safe).not.toContain(secret ?? "missing-secret");
      expect(safe).toContain("trace-marker");
    }
    expect(redactDebug({ joinCode: null })).toEqual({ joinCode: null });
  });
  it("事件只保留最近 100 条，且存储时已经脱敏", () => {
    for (let n = 0; n < 150; n++)
      recordDebug("test", { n, joinCode: "secret-room" });
    const events = debugEvents();
    expect(events).toHaveLength(100);
    expect(events[0]?.data).toEqual({ n: 50, joinCode: "[redacted]" });
    expect(JSON.stringify(events)).not.toContain("secret-room");
    clearDebugEvents();
    expect(debugEvents()).toEqual([]);
  });
  it("诊断验证不会裁掉后端新增的原始字段", () => {
    const raw = {
      version: "1",
      snapshot: { custom: 7 },
      endpointAddressRaw: "address",
      boundSockets: [],
      connections: [
        {
          remoteId: "remote",
          statsRaw: "raw",
          future: 42,
          paths: [{ statsRaw: "path", custom: true }],
        },
      ],
      localWebRtc: {},
      actor: {},
      extra: 9,
    };
    expect(diagnosticsSchema.parse(raw)).toEqual(raw);
    expect(
      diagnosticsSchema.safeParse({ ...raw, connections: "invalid" }).success,
    ).toBe(false);
  });
});
