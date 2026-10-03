import { Button } from "@cloudflare/kumo/components/button";
import { Surface } from "@cloudflare/kumo/components/surface";
import { Dialog } from "@cloudflare/kumo/components/dialog";
import { DebugPage } from "./debug/DebugPage";
import { RoomEntry } from "./components/RoomEntry";
import { RoomPanel } from "./components/RoomPanel";
import { QualityDialog } from "./components/QualityDialog";
import { useSession } from "./session/useSession";
import { FilesPage } from "./files/FilesPage";
import { MonitorIcon, FolderOpenIcon, CodeIcon } from "@phosphor-icons/react";

export default function App() {
  const session = useSession();
  const {
    qualityOpen,
    ownCapture,
    captureStream,
    busy,
    openQuality,
    setQualityOpen,
    debug,
    page,
    setPage,
    token,
    readDebugMedia,
    readDebugRuntime,
    room,
    error,
    notice,
    setNotice,
  } = session;
  const pendingFiles = Object.values(session.snapshot?.files ?? {}).filter(
    (file) =>
      file.recipientId === session.snapshot?.endpointId &&
      file.state === "offered" &&
      !session.snapshot?.fileClients[file.id],
  ).length;
  return (
    <Dialog.Root
      open={qualityOpen && !!ownCapture && !!captureStream}
      onOpenChange={(open) => {
        if (!busy) {
          if (open) openQuality();
          else setQualityOpen(false);
        }
      }}
    >
      <div className="min-h-screen bg-kumo-base text-kumo-default">
        <header className="border-b border-kumo-line">
          <div className="mx-auto flex max-w-6xl flex-wrap items-center justify-between gap-x-6 gap-y-3 px-5 py-4">
            <div className="flex items-center gap-3">
              <h1 className="text-xl font-semibold text-kumo-strong">
                LastOrder
              </h1>
              <span
                role="status"
                className="inline-flex items-center gap-1.5 text-sm text-kumo-subtle"
              >
                <span
                  aria-hidden="true"
                  className={`size-2 rounded-full ${session.connected ? "bg-kumo-success" : "bg-kumo-line"}`}
                />
                {!token
                  ? "未授权"
                  : session.connected
                    ? "本地已连接"
                    : "本地未连接"}
              </span>
            </div>
            <nav
              aria-label="主要页面"
              className="flex w-full flex-wrap gap-1 rounded-xl bg-kumo-control/50 p-1 ring ring-kumo-line sm:w-auto"
            >
              {(
                [
                  ["/", "屏幕共享", MonitorIcon],
                  ["/files", "文件传输", FolderOpenIcon],
                  ["/debug", "Debug", CodeIcon],
                ] as const
              ).map(([path, label, Icon]) => (
                <a
                  key={path}
                  href={path}
                  aria-current={page === path ? "page" : undefined}
                  aria-label={
                    path === "/files" && pendingFiles
                      ? `${label}，${pendingFiles} 个待接收邀请`
                      : label
                  }
                  className={`inline-flex min-w-0 flex-1 items-center justify-center gap-2 rounded-lg px-3 py-2 text-sm whitespace-nowrap focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-kumo-brand sm:flex-none ${page === path ? "bg-kumo-base font-medium text-kumo-strong ring ring-kumo-line" : "text-kumo-subtle hover:bg-kumo-control hover:text-kumo-default"}`}
                  onClick={(event) => {
                    if (
                      event.ctrlKey ||
                      event.metaKey ||
                      event.shiftKey ||
                      event.altKey
                    )
                      return;
                    event.preventDefault();
                    window.history.pushState(null, "", path);
                    setPage(path);
                    setQualityOpen(false);
                  }}
                >
                  <Icon
                    size={16}
                    aria-hidden="true"
                    className="hidden shrink-0 sm:block"
                  />
                  {label}
                  {path === "/files" && pendingFiles > 0 && (
                    <span
                      aria-hidden="true"
                      className="rounded-md bg-kumo-info-tint px-1.5 text-sm font-medium text-kumo-info tabular-nums"
                    >
                      {pendingFiles}
                    </span>
                  )}
                </a>
              ))}
            </nav>
          </div>
        </header>
        {debug && (
          <DebugPage
            token={token}
            readMedia={readDebugMedia}
            readRuntime={readDebugRuntime}
          />
        )}
        <main
          className={`mx-auto max-w-6xl gap-6 px-5 py-6 ${debug ? "hidden" : "grid"}`}
          aria-hidden={debug}
        >
          {!token && (
            <Surface className="rounded-xl px-5 py-4 ring ring-kumo-line">
              <p>
                请使用 CLI 输出的本地地址打开页面，其中包含本次运行的访问凭证。
              </p>
            </Surface>
          )}
          {error && (
            <div
              role="alert"
              className="flex items-start justify-between gap-3 rounded-lg bg-kumo-danger-tint px-4 py-3 text-sm text-kumo-danger"
            >
              <span>{error}</span>
              {notice && (
                <Button
                  variant="ghost"
                  size="xs"
                  onClick={() => setNotice(null)}
                >
                  关闭
                </Button>
              )}
            </div>
          )}
          <div
            className={page === "/" ? "grid" : "hidden"}
            aria-hidden={page !== "/"}
          >
            {!room ? (
              <RoomEntry session={session} />
            ) : (
              <RoomPanel session={session} />
            )}
          </div>
          <div
            className={page === "/files" ? "grid" : "hidden"}
            aria-hidden={page !== "/files"}
          >
            <FilesPage
              key={`${room?.id ?? "none"}:${session.clientId}`}
              session={session}
            />
          </div>
        </main>
        <QualityDialog session={session} />
      </div>
    </Dialog.Root>
  );
}
