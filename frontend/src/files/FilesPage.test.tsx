import { describe, expect, it } from "bun:test";
import { renderToStaticMarkup } from "react-dom/server";
import type { ComponentProps } from "react";
import type { SharedFile, Snapshot } from "../api";
import { FilesPage } from "./FilesPage";

type FileSession = ComponentProps<typeof FilesPage>["session"];
const incoming: SharedFile = {
  id: "incoming",
  roomId: "room",
  name: "报告.pdf",
  size: 2048,
  publisherId: "peer",
  recipientId: "self",
  state: "offered",
  blobTicket: null,
};

function makeSession(files: SharedFile[] = []): FileSession {
  const snapshot: Snapshot = {
    eventSeq: 1,
    endpointId: "self",
    room: {
      id: "room",
      ownerId: "self",
      revision: 1,
      members: {
        self: { id: "self", name: "我" },
        peer: { id: "peer", name: "小林" },
      },
      share: null,
    },
    files: Object.fromEntries(files.map((file) => [file.id, file])),
    fileClients: {},
    joinCode: null,
    joinTicket: null,
    discoveryError: null,
    capture: null,
    subscription: null,
    connection: null,
    networkBitrate: null,
    error: null,
  };
  return {
    snapshot,
    room: snapshot.room,
    connected: true,
    token: "test-token",
    clientId: "page",
    setNotice: () => {},
    setPage: () => {},
  };
}

async function render(session: FileSession): Promise<{
  html: string;
  actions: { label: string; disabled: boolean }[];
  pickerDisabled: boolean | null;
}> {
  const html = renderToStaticMarkup(<FilesPage session={session} />);
  const actions: { label: string; disabled: boolean }[] = [];
  let pickerDisabled: boolean | null = null;
  await new HTMLRewriter()
    .on("ul[aria-label] button[aria-label]", {
      element(element) {
        const label = element.getAttribute("aria-label");
        if (label !== null) {
          actions.push({
            label,
            disabled: element.getAttribute("disabled") !== null,
          });
        }
      },
    })
    .on('button[aria-labelledby="file-picker-label"]', {
      element(element) {
        pickerDisabled = element.getAttribute("disabled") !== null;
      },
    })
    .transform(new Response(html))
    .text();
  return { html, actions, pickerDisabled };
}

describe("文件传输页面", () => {
  it("待确认的邀请排在记录前，并提供接收和拒绝操作", async () => {
    const completed: SharedFile = {
      ...incoming,
      id: "completed",
      name: "完成.zip",
      state: "completed",
    };
    const { html, actions } = await render(makeSession([completed, incoming]));
    expect(html.indexOf("待你确认")).toBeLessThan(html.indexOf("传输记录"));
    expect(html).toContain("等待你确认");
    expect(html).toContain("2.0 KiB");
    expect(html).toContain("小林");
    expect(actions).toEqual([
      { label: "接收 报告.pdf", disabled: false },
      { label: "拒绝 报告.pdf", disabled: false },
    ]);
  });

  it("已由其他页面处理的邀请不再提供可用的确认操作", async () => {
    const session = makeSession([incoming]);
    if (!session.snapshot) throw new Error("测试快照不存在");
    session.snapshot.fileClients[incoming.id] = "another-page";
    const { html, actions } = await render(session);
    expect(html).toContain("正在处理邀请");
    expect(actions).toEqual([
      { label: "接收 报告.pdf", disabled: true },
      { label: "拒绝 报告.pdf", disabled: true },
    ]);
  });

  it("发送方只能撤回待确认的邀请，不会看到接收方操作", async () => {
    const outgoing: SharedFile = {
      ...incoming,
      publisherId: "self",
      recipientId: "peer",
    };
    const { html, actions } = await render(makeSession([outgoing]));
    expect(html).not.toContain("待你确认");
    expect(html).toContain("等待对方确认");
    expect(actions).toEqual([{ label: "取消 报告.pdf", disabled: false }]);
  });

  it("连接断开时禁用确认操作，未选择对象时禁用文件选择", async () => {
    const session = { ...makeSession([incoming]), connected: false };
    const { actions, pickerDisabled } = await render(session);
    expect(actions.every((action) => action.disabled)).toBe(true);
    expect(pickerDisabled).toBe(true);
    expect((await render(makeSession())).pickerDisabled).toBe(true);
  });

  it("结束的记录显示状态而非操作，且不会把 Ticket 显示到页面", async () => {
    const terminalStates = ["completed", "rejected", "cancelled"] as const;
    const files: SharedFile[] = terminalStates.map((state) => ({
      ...incoming,
      id: state,
      state,
      blobTicket: "private-test-ticket",
    }));
    const { html, actions } = await render(makeSession(files));
    expect(html).toContain("已完成");
    expect(html).toContain("已拒绝");
    expect(html).toContain("已取消");
    expect(html).not.toContain("private-test-ticket");
    expect(actions).toEqual([]);
  });

  it("没有房间时给出加入房间入口，不显示不可用的选择表单", async () => {
    const session = makeSession();
    session.room = null;
    session.snapshot = null;
    const { html, pickerDisabled } = await render(session);
    expect(html).toContain("去加入房间");
    expect(html).toContain("还没有传输记录");
    expect(pickerDisabled).toBeNull();
  });
});
