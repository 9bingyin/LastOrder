import { Surface } from "@cloudflare/kumo/components/surface";
import { Input } from "@cloudflare/kumo/components/input";
import { Button } from "@cloudflare/kumo/components/button";
import { ArrowRightIcon } from "@phosphor-icons/react";
import type { SessionState } from "../session/useSession";

export function RoomEntry({ session }: { session: SessionState }) {
  const {
    name,
    setName,
    available,
    busy,
    perform,
    api,
    joinCode,
    setJoinCode,
  } = session;
  return (
    <div className="mx-auto grid w-full max-w-xl gap-5 py-8">
      <div className="grid gap-1.5">
        <h2 className="text-2xl font-semibold text-kumo-strong">
          一起看点什么
        </h2>
      </div>
      <Surface className="grid gap-5 rounded-xl px-5 py-5 ring ring-kumo-line">
        <Input
          label="你的昵称"
          value={name}
          maxLength={32}
          onChange={(event) => setName(event.target.value)}
        />
        <Button
          variant="primary"
          disabled={!available || !name.trim()}
          loading={busy}
          onClick={() =>
            void perform(async () => {
              await api("/room", "POST", { displayName: name });
            })
          }
        >
          创建房间
        </Button>
        <div className="border-t border-kumo-line pt-4">
          <div className="grid gap-3">
            <Input
              label="加入码"
              placeholder="输入 UUID 或 Ticket"
              value={joinCode}
              maxLength={4096}
              onChange={(event) => setJoinCode(event.target.value)}
            />
            <Button
              icon={ArrowRightIcon}
              disabled={!available || !joinCode.trim() || !name.trim()}
              onClick={() =>
                void perform(async () => {
                  await api("/room/join", "POST", {
                    joinCode: joinCode.trim(),
                    displayName: name,
                  });
                })
              }
            >
              加入房间
            </Button>
          </div>
        </div>
      </Surface>
    </div>
  );
}
