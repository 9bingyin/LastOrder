import { Surface } from "@cloudflare/kumo/components/surface";
import { Input } from "@cloudflare/kumo/components/input";
import { Button } from "@cloudflare/kumo/components/button";
import { ArrowRightIcon, PlusIcon } from "@phosphor-icons/react";
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
  const join = () =>
    void perform(async () => {
      await api("/room/join", "POST", {
        joinCode: joinCode.trim(),
        displayName: name,
      });
    });
  const canJoin = available && !!joinCode.trim() && !!name.trim();
  return (
    <div className="mx-auto grid w-full max-w-md py-10">
      <Surface className="grid gap-5 rounded-xl px-5 py-5 ring ring-kumo-line">
        <Input
          label="你的昵称"
          value={name}
          maxLength={32}
          onChange={(event) => setName(event.target.value)}
        />
        <Button
          variant="primary"
          icon={PlusIcon}
          className="w-full justify-center"
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
        <div
          role="separator"
          className="flex items-center gap-3 text-sm text-kumo-subtle before:h-px before:flex-1 before:bg-kumo-line after:h-px after:flex-1 after:bg-kumo-line"
        >
          或加入已有房间
        </div>
        <form
          className="flex items-end gap-2"
          onSubmit={(event) => {
            event.preventDefault();
            if (canJoin) join();
          }}
        >
          <div className="min-w-0 flex-1">
            <Input
              label="加入码"
              placeholder="输入 UUID 或 Ticket"
              value={joinCode}
              maxLength={4096}
              onChange={(event) => setJoinCode(event.target.value)}
            />
          </div>
          <Button type="submit" icon={ArrowRightIcon} disabled={!canJoin}>
            加入
          </Button>
        </form>
      </Surface>
    </div>
  );
}
