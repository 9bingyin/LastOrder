import { Button } from "@cloudflare/kumo/components/button";
import { Surface } from "@cloudflare/kumo/components/surface";
import { Dialog } from "@cloudflare/kumo/components/dialog";
import { DebugPage } from "./debug/DebugPage";
import { RoomEntry } from "./components/RoomEntry";
import { RoomPanel } from "./components/RoomPanel";
import { QualityDialog } from "./components/QualityDialog";
import { useSession } from "./session/useSession";

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
    setDebug,
    token,
    readDebugMedia,
    readDebugRuntime,
    room,
    error,
    notice,
    setNotice,
  } = session;
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
          <div className="mx-auto flex max-w-6xl items-center justify-between gap-4 px-5 py-4">
            <h1 className="text-xl font-semibold text-kumo-strong">
              LastOrder
            </h1>
            <a
              href={debug ? "/" : "/debug"}
              className="text-sm text-kumo-subtle hover:text-kumo-default"
              onClick={(event) => {
                if (
                  event.ctrlKey ||
                  event.metaKey ||
                  event.shiftKey ||
                  event.altKey
                )
                  return;
                event.preventDefault();
                window.history.pushState(null, "", debug ? "/" : "/debug");
                setDebug(!debug);
                setQualityOpen(false);
              }}
            >
              {debug ? "返回" : "Debug"}
            </a>
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
          className={`mx-auto max-w-6xl gap-6 px-5 py-7 ${debug ? "hidden" : "grid"}`}
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
          {!room ? (
            <RoomEntry session={session} />
          ) : (
            <RoomPanel session={session} />
          )}
        </main>
        <QualityDialog session={session} />
      </div>
    </Dialog.Root>
  );
}
