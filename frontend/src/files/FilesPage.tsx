import { useEffect, useRef, useState } from "react";
import { Button } from "@cloudflare/kumo/components/button";
import { Surface } from "@cloudflare/kumo/components/surface";
import { Loader } from "@cloudflare/kumo/components/loader";
import {
  ArrowDownLeftIcon,
  ArrowUpRightIcon,
  CheckCircleIcon,
  FileIcon,
  PaperPlaneTiltIcon,
  TrayIcon,
  UploadSimpleIcon,
  XIcon,
} from "@phosphor-icons/react";
import type { SessionState } from "../session/useSession";
import type { SharedFile } from "../api";
import {
  changeFile,
  chooseSaveFile,
  downloadFile,
  offerFile,
  transferError,
  uploadFile,
} from "./transfers";
import type { FileCredentials, SaveTarget } from "./transfers";

type Job = { controller: AbortController; phase: string };
const activeState = (file: SharedFile) =>
  ["offered", "accepted", "ready"].includes(file.state);
const labels = {
  offered: "等待对方确认",
  accepted: "正在准备",
  ready: "文件已就绪",
  completed: "已完成",
  rejected: "已拒绝",
  cancelled: "已取消",
} satisfies Record<SharedFile["state"], string>;

function formatSize(size: number): string {
  const units = ["B", "KiB", "MiB", "GiB", "TiB", "PiB", "EiB"];
  let unit = 0;
  while (size >= 1024 && unit < units.length - 1) {
    size /= 1024;
    unit++;
  }
  return `${unit ? size.toFixed(1) : size} ${units[unit]}`;
}

type FileSession = Pick<
  SessionState,
  | "room"
  | "snapshot"
  | "token"
  | "clientId"
  | "connected"
  | "setNotice"
  | "setPage"
>;
type TransferFilter = "all" | "active" | "ended";

export function FilesPage({ session }: { session: FileSession }) {
  const { room, snapshot, token, clientId, connected, setNotice } = session;
  const [recipient, setRecipient] = useState("");
  const [picked, setPicked] = useState<File[]>([]);
  const [sending, setSending] = useState(false);
  const [dragging, setDragging] = useState(false);
  const [filter, setFilter] = useState<TransferFilter>("all");
  const [revision, refresh] = useState(0);
  const input = useRef<HTMLInputElement>(null);
  const sources = useRef(new Map<string, File>());
  const targets = useRef(new Map<string, SaveTarget | null>());
  const jobs = useRef(new Map<string, Job>());
  const failures = useRef(new Map<string, string>());
  const mounted = useRef(true);
  const invitation = useRef<AbortController | null>(null);
  const credentials: FileCredentials | null =
    token && clientId && room && connected
      ? { token, clientId, roomId: room.id }
      : null;

  useEffect(() => {
    mounted.current = true;
    const active = jobs.current;
    return () => {
      mounted.current = false;
      invitation.current?.abort();
      for (const job of active.values()) job.controller.abort();
      active.clear();
    };
  }, []);

  function update() {
    if (mounted.current) refresh((value) => value + 1);
  }

  async function run(
    id: string,
    phase: string,
    action: (auth: FileCredentials, signal: AbortSignal) => Promise<void>,
  ) {
    if (!credentials || jobs.current.has(id)) return;
    const controller = new AbortController();
    jobs.current.set(id, { controller, phase });
    failures.current.delete(id);
    update();
    try {
      await action(credentials, controller.signal);
    } catch (error) {
      const message = transferError(error);
      if (mounted.current && message) failures.current.set(id, message);
    } finally {
      jobs.current.delete(id);
      update();
    }
  }

  useEffect(() => {
    if (!snapshot || !credentials) return;
    for (const file of Object.values(snapshot.files)) {
      if (!activeState(file)) {
        jobs.current.get(file.id)?.controller.abort();
        sources.current.delete(file.id);
        targets.current.delete(file.id);
        failures.current.delete(file.id);
        continue;
      }
      if (
        snapshot.fileClients[file.id] !== clientId ||
        jobs.current.has(file.id) ||
        failures.current.has(file.id)
      )
        continue;
      const source = sources.current.get(file.id);
      if (
        file.publisherId === snapshot.endpointId &&
        file.state === "accepted" &&
        source
      ) {
        void run(file.id, "正在准备文件…", (auth, signal) =>
          uploadFile(auth, file.id, source, signal),
        );
      } else if (
        file.recipientId === snapshot.endpointId &&
        file.state === "ready" &&
        targets.current.has(file.id)
      ) {
        const target = targets.current.get(file.id) ?? null;
        void run(file.id, "正在接收文件…", async (auth, signal) => {
          await downloadFile(
            auth,
            file,
            signal,
            () => {
              const job = jobs.current.get(file.id);
              if (job) {
                job.phase = "正在保存文件…";
                update();
              }
            },
            target,
          );
          await changeFile(auth, file.id, "completed", signal);
        });
      }
    }
  }, [snapshot, clientId, connected, revision]);

  async function send() {
    if (!credentials || !recipient || !picked.length || sending) return;
    const controller = new AbortController();
    invitation.current = controller;
    setSending(true);
    let sent = 0;
    try {
      for (const source of picked) {
        const offer = await offerFile(
          credentials,
          recipient,
          source,
          controller.signal,
        );
        sources.current.set(offer.id, source);
        sent++;
        update();
      }
    } catch (error) {
      const message = transferError(error);
      if (mounted.current && message) setNotice(message);
    } finally {
      if (mounted.current) {
        setPicked((files) => files.slice(sent));
        setSending(false);
      }
      invitation.current = null;
    }
  }

  function accept(file: SharedFile) {
    void run(file.id, "正在确认接收…", async (auth, signal) => {
      const target = await chooseSaveFile(file);
      signal.throwIfAborted();
      targets.current.set(file.id, target);
      await changeFile(auth, file.id, "accepted", signal);
    });
  }

  async function cancel(file: SharedFile) {
    if (!credentials) return;
    failures.current.set(file.id, "");
    jobs.current.get(file.id)?.controller.abort();
    update();
    try {
      await changeFile(
        credentials,
        file.id,
        "cancelled",
        new AbortController().signal,
      );
    } catch (error) {
      if (mounted.current) setNotice(transferError(error));
    }
  }

  const members = Object.values(room?.members ?? {}).filter(
    (member) => member.id !== snapshot?.endpointId,
  );
  const selectedMember = members.find((member) => member.id === recipient);
  const canPick = connected && !!selectedMember && !sending;
  const files = Object.values(snapshot?.files ?? {});
  const receiving = files.some(
    (file) => file.recipientId === snapshot?.endpointId && activeState(file),
  );
  const needsAccept = (file: SharedFile) =>
    file.recipientId === snapshot?.endpointId && file.state === "offered";
  const pending = files.filter(needsAccept);
  const records = files.filter((file) => !needsAccept(file));
  const visible = records.filter(
    (file) =>
      filter === "all" ||
      (filter === "active" ? activeState(file) : !activeState(file)),
  );
  const pickedSize = picked.reduce((total, file) => total + file.size, 0);
  const filters = [
    { value: "all", label: "全部", count: records.length },
    {
      value: "active",
      label: "进行中",
      count: records.filter(activeState).length,
    },
    {
      value: "ended",
      label: "已结束",
      count: records.filter((file) => !activeState(file)).length,
    },
  ] satisfies { value: TransferFilter; label: string; count: number }[];

  function renderTransfer(file: SharedFile) {
    const incoming = file.recipientId === snapshot?.endpointId;
    const waiting = needsAccept(file);
    const other = incoming ? file.publisherId : file.recipientId;
    const job = jobs.current.get(file.id);
    const failed = failures.current.get(file.id);
    const claimed = !!snapshot?.fileClients[file.id];
    const cancelling = failed === "";
    const working = !!job || cancelling || file.state === "accepted";
    const status = cancelling
      ? "正在取消…"
      : (job?.phase ??
        (failed
          ? "传输失败"
          : waiting
            ? claimed
              ? "正在处理邀请"
              : "等待你确认"
            : labels[file.state]));
    const tone = failed
      ? "text-kumo-danger"
      : file.state === "completed"
        ? "text-kumo-success"
        : waiting || working || file.state === "ready"
          ? "text-kumo-info"
          : "text-kumo-subtle";
    const DirectionIcon = incoming ? ArrowDownLeftIcon : ArrowUpRightIcon;

    return (
      <li
        key={file.id}
        className="flex min-w-0 flex-wrap items-center gap-x-3 gap-y-2 px-5 py-3.5"
      >
        <span
          aria-hidden="true"
          className={`flex size-8 shrink-0 items-center justify-center rounded-lg ${
            incoming
              ? "bg-kumo-info-tint text-kumo-info"
              : "bg-kumo-control text-kumo-subtle"
          }`}
        >
          <DirectionIcon size={16} />
        </span>
        <div className="grid min-w-0 flex-1 basis-48 gap-0.5">
          <p className="text-sm font-medium [overflow-wrap:anywhere]">
            {file.name}
          </p>
          <p className="flex flex-wrap items-center gap-x-1.5 text-sm text-kumo-subtle">
            <span className="tabular-nums">{formatSize(file.size)}</span>
            <span aria-hidden="true">·</span>
            <span className="[overflow-wrap:anywhere]">
              {incoming ? "来自" : "发送给"}{" "}
              {room?.members[other]?.name ?? "已离开的成员"}
            </span>
            <span aria-hidden="true">·</span>
            <span
              role="status"
              className={`inline-flex items-center gap-1 font-medium ${tone}`}
            >
              {working && !failed ? (
                <Loader size={12} />
              ) : file.state === "completed" ? (
                <CheckCircleIcon size={14} aria-hidden="true" />
              ) : null}
              {status}
            </span>
          </p>
          {failed && (
            <p
              role="alert"
              className="text-sm text-kumo-danger [overflow-wrap:anywhere]"
            >
              {failed}
            </p>
          )}
        </div>
        <div className="flex shrink-0 flex-wrap items-center gap-2 max-sm:pl-11">
          {waiting && (
            <>
              <Button
                size="sm"
                variant="primary"
                disabled={!connected || !!job || claimed || cancelling}
                aria-label={`接收 ${file.name}`}
                onClick={() => accept(file)}
              >
                接收
              </Button>
              <Button
                size="sm"
                variant="secondary"
                disabled={!connected || !!job || claimed || cancelling}
                aria-label={`拒绝 ${file.name}`}
                onClick={() =>
                  void run(file.id, "正在拒绝…", (auth, signal) =>
                    changeFile(auth, file.id, "rejected", signal),
                  )
                }
              >
                拒绝
              </Button>
            </>
          )}
          {failed && ["accepted", "ready"].includes(file.state) && !job && (
            <Button
              size="sm"
              disabled={!connected}
              aria-label={`重试 ${file.name}`}
              onClick={() => {
                failures.current.delete(file.id);
                update();
              }}
            >
              重试
            </Button>
          )}
          {activeState(file) && (!waiting || !!job) && (
            <Button
              size="sm"
              variant="ghost"
              disabled={!connected || cancelling}
              aria-label={`取消 ${file.name}`}
              onClick={() => void cancel(file)}
            >
              取消
            </Button>
          )}
        </div>
      </li>
    );
  }

  return (
    <div
      className="grid gap-5"
      onDragOver={(event) => {
        if (event.dataTransfer.types.includes("Files")) event.preventDefault();
      }}
      onDrop={(event) => {
        if (event.dataTransfer.types.includes("Files")) {
          event.preventDefault();
          setDragging(false);
        }
      }}
    >
      <div className="flex flex-wrap items-center justify-between gap-3">
        <h2 className="text-xl font-semibold text-kumo-strong">文件传输</h2>
      </div>

      <div className="grid min-w-0 items-start gap-5 lg:grid-cols-[320px_minmax(0,1fr)]">
        <Surface
          className={`grid min-w-0 gap-4 rounded-xl px-5 py-4 ring ring-kumo-line ${
            receiving ? "order-2 lg:order-none" : ""
          }`}
        >
          {!room ? (
            <div className="grid justify-items-start gap-3">
              <p className="text-sm text-kumo-subtle">
                先创建或加入房间，即可选择接收对象并发送文件。
              </p>
              <Button
                size="sm"
                disabled={!token}
                onClick={() => {
                  window.history.pushState(null, "", "/");
                  session.setPage("/");
                }}
              >
                去加入房间
              </Button>
            </div>
          ) : (
            <>
              <div className="grid gap-2">
                <span id="recipient-label" className="text-sm font-medium">
                  接收成员
                </span>
                {members.length ? (
                  <div
                    role="radiogroup"
                    aria-labelledby="recipient-label"
                    className="flex flex-wrap gap-1.5"
                  >
                    {members.map((member) => {
                      const selected = recipient === member.id;
                      return (
                        <button
                          key={member.id}
                          type="button"
                          role="radio"
                          aria-checked={selected}
                          disabled={!connected || sending}
                          onClick={() => setRecipient(member.id)}
                          className={`rounded-full px-3 py-1 text-sm ring focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-kumo-brand disabled:cursor-not-allowed disabled:opacity-50 ${
                            selected
                              ? "bg-kumo-brand font-medium text-white ring-kumo-brand"
                              : "bg-kumo-control/50 ring-kumo-line enabled:hover:bg-kumo-control"
                          }`}
                        >
                          {member.name}
                        </button>
                      );
                    })}
                  </div>
                ) : (
                  <p className="text-sm text-kumo-subtle">
                    房间里还没有其他成员。
                  </p>
                )}
              </div>

              <div className="grid gap-2">
                <span id="file-picker-label" className="text-sm font-medium">
                  选择文件
                </span>
                <button
                  type="button"
                  disabled={!canPick}
                  aria-labelledby="file-picker-label"
                  aria-describedby="file-picker-hint"
                  className={`grid min-h-28 content-center justify-items-center gap-1.5 rounded-lg border border-dashed px-4 py-4 text-sm focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-kumo-brand disabled:cursor-not-allowed disabled:opacity-50 ${
                    dragging
                      ? "border-kumo-brand bg-kumo-brand/10"
                      : "border-kumo-line enabled:hover:bg-kumo-control/40"
                  }`}
                  onClick={() => input.current?.click()}
                  onDragEnter={(event) => {
                    event.preventDefault();
                    if (canPick && event.dataTransfer.types.includes("Files"))
                      setDragging(true);
                  }}
                  onDragOver={(event) => {
                    event.preventDefault();
                    event.dataTransfer.dropEffect = canPick ? "copy" : "none";
                  }}
                  onDragLeave={(event) => {
                    if (
                      !(event.relatedTarget instanceof Node) ||
                      !event.currentTarget.contains(event.relatedTarget)
                    )
                      setDragging(false);
                  }}
                  onDrop={(event) => {
                    event.preventDefault();
                    setDragging(false);
                    if (!canPick) return;
                    if (
                      Array.from(event.dataTransfer.items).some(
                        (item) => item.webkitGetAsEntry?.()?.isDirectory,
                      )
                    ) {
                      setNotice("暂不支持文件夹，请选择普通文件");
                      return;
                    }
                    const selected = Array.from(event.dataTransfer.files);
                    if (selected.length) setPicked(selected);
                  }}
                >
                  <UploadSimpleIcon
                    size={22}
                    aria-hidden="true"
                    className="pointer-events-none text-kumo-subtle"
                  />
                  <span className="pointer-events-none font-medium">
                    {dragging ? "松开即可添加" : "拖入或点击选择文件"}
                  </span>
                  <span
                    id="file-picker-hint"
                    className="pointer-events-none text-kumo-subtle"
                  >
                    {selectedMember ? "支持多选文件" : "请先选择接收成员"}
                  </span>
                </button>
                <input
                  ref={input}
                  className="hidden"
                  type="file"
                  multiple
                  aria-label="选择要发送的文件"
                  onChange={(event) => {
                    setPicked(Array.from(event.currentTarget.files ?? []));
                    event.currentTarget.value = "";
                  }}
                />
              </div>

              {picked.length > 0 && (
                <div className="grid gap-2">
                  <div className="flex items-center justify-between text-sm text-kumo-subtle">
                    <span className="tabular-nums font-medium text-kumo-strong">
                      已选 {picked.length} 个文件 · {formatSize(pickedSize)}
                    </span>
                    <Button
                      size="xs"
                      variant="ghost"
                      disabled={sending}
                      onClick={() => setPicked([])}
                    >
                      清空
                    </Button>
                  </div>
                  <ul className="max-h-52 divide-y divide-kumo-line overflow-y-auto rounded-lg ring ring-kumo-line">
                    {picked.map((file, index) => (
                      <li
                        key={`${file.name}:${index}`}
                        className="flex items-center gap-2 px-3 py-2 text-sm"
                      >
                        <FileIcon
                          size={16}
                          aria-hidden="true"
                          className="shrink-0 text-kumo-subtle"
                        />
                        <span
                          className="min-w-0 flex-1 truncate font-medium"
                          title={file.name}
                        >
                          {file.name}
                        </span>
                        <span className="shrink-0 text-kumo-subtle tabular-nums">
                          {formatSize(file.size)}
                        </span>
                        <Button
                          size="xs"
                          variant="ghost"
                          shape="square"
                          icon={XIcon}
                          aria-label={`移除 ${file.name}`}
                          disabled={sending}
                          onClick={() =>
                            setPicked((current) =>
                              current.filter((_, position) => position !== index),
                            )
                          }
                        />
                      </li>
                    ))}
                  </ul>
                </div>
              )}

              <Button
                className="w-full justify-center"
                variant="primary"
                icon={PaperPlaneTiltIcon}
                loading={sending}
                disabled={!canPick || !picked.length}
                onClick={() => void send()}
              >
                {sending ? "正在发送邀请…" : "发送邀请"}
              </Button>
            </>
          )}
        </Surface>

        <div
          className={`grid min-w-0 gap-5 ${
            receiving ? "order-1 lg:order-none" : ""
          }`}
        >
          {pending.length > 0 && (
            <Surface className="min-w-0 overflow-hidden rounded-xl ring ring-kumo-info/30">
              <h3 className="flex items-center gap-2 border-b border-kumo-line bg-kumo-info-tint/40 px-5 py-3 text-base font-semibold text-kumo-strong">
                待你确认
                <span className="rounded-full bg-kumo-info px-2 text-sm font-medium text-white tabular-nums">
                  {pending.length}
                </span>
              </h3>
              <ul
                aria-label="待接收的文件邀请"
                className="divide-y divide-kumo-line"
              >
                {pending.map(renderTransfer)}
              </ul>
            </Surface>
          )}

          <Surface className="min-w-0 overflow-hidden rounded-xl ring ring-kumo-line">
            <div className="flex flex-wrap items-center justify-between gap-3 border-b border-kumo-line px-5 py-3">
              <h3 className="text-base font-semibold text-kumo-strong">
                传输记录
              </h3>
              <div
                role="group"
                aria-label="筛选传输记录"
                className="flex gap-1 rounded-lg bg-kumo-control/50 p-0.5 ring ring-kumo-line"
              >
                {filters.map((item) => (
                  <button
                    key={item.value}
                    type="button"
                    aria-pressed={filter === item.value}
                    onClick={() => setFilter(item.value)}
                    className={`inline-flex items-center gap-1.5 rounded-md px-2.5 py-1 text-sm focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-kumo-brand ${
                      filter === item.value
                        ? "bg-kumo-base font-medium text-kumo-strong ring ring-kumo-line"
                        : "text-kumo-subtle hover:text-kumo-default"
                    }`}
                  >
                    {item.label}
                    <span className="tabular-nums text-kumo-subtle">
                      {item.count}
                    </span>
                  </button>
                ))}
              </div>
            </div>
            {visible.length > 0 ? (
              <ul
                aria-label="文件传输记录"
                className="divide-y divide-kumo-line"
              >
                {visible.map(renderTransfer)}
              </ul>
            ) : (
              <div className="grid justify-items-center gap-2 px-5 py-12 text-center text-sm text-kumo-subtle">
                <TrayIcon size={32} aria-hidden="true" />
                <p className="font-medium text-kumo-default">
                  {filter === "all"
                    ? "还没有传输记录"
                    : filter === "active"
                      ? "暂无进行中的传输"
                      : "暂无已结束的传输"}
                </p>
                <p className="max-w-sm">
                  {filter === "all"
                    ? "向成员发送文件邀请，或等待接收对方发送的文件。"
                    : "可切换上方筛选查看其他记录。"}
                </p>
              </div>
            )}
          </Surface>
        </div>
      </div>
    </div>
  );
}
