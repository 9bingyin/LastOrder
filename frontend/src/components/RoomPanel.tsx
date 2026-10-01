import { Surface } from "@cloudflare/kumo/components/surface";
import { Button } from "@cloudflare/kumo/components/button";
import { Badge } from "@cloudflare/kumo/components/badge";
import { CopyIcon, UsersIcon } from "@phosphor-icons/react";
import { Player } from "./Player";
import type { SessionState } from "../session/useSession";

export function RoomPanel({ session }: { session: SessionState }) {
  const {
    room,
    snapshot,
    available,
    perform,
    clearCapture,
    clearWatching,
    api,
  } = session;
  if (!room) return null;
  return (
    <>
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div className="grid gap-1.5">
          <h2 className="text-xl font-semibold text-kumo-strong">房间</h2>
          <div className="flex flex-wrap items-center gap-3 text-sm text-kumo-subtle">
            <span className="flex items-center gap-1.5">
              <UsersIcon size={16} />
              {Object.keys(room.members).length} / 5 人
            </span>
          </div>
        </div>
        <div className="flex flex-wrap gap-2">
          {snapshot?.joinCode && (
            <Button
              icon={CopyIcon}
              disabled={!available}
              onClick={() =>
                void perform(async () => {
                  await navigator.clipboard.writeText(snapshot.joinCode ?? "");
                })
              }
            >
              复制加入码
            </Button>
          )}
          {snapshot?.joinTicket && (
            <Button
              icon={CopyIcon}
              disabled={!available}
              onClick={() =>
                void perform(async () => {
                  await navigator.clipboard.writeText(
                    snapshot.joinTicket ?? "",
                  );
                })
              }
            >
              复制 Ticket
            </Button>
          )}
          <Button
            variant="secondary-destructive"
            disabled={!available}
            onClick={() =>
              void perform(async () => {
                clearCapture();
                clearWatching();
                await api("/room", "DELETE");
              })
            }
          >
            离开房间
          </Button>
        </div>
      </div>
      <div className="grid items-start gap-5 lg:grid-cols-[minmax(0,1fr)_260px]">
        <div className="grid gap-4">
          <Player session={session} />
        </div>
        <Surface className="grid gap-4 rounded-xl px-5 py-4 ring ring-kumo-line">
          <h3 className="text-base font-semibold text-kumo-strong">房间成员</h3>
          <ul className="grid gap-4">
            {Object.values(room.members).map((member) => (
              <li key={member.id} className="grid gap-1">
                <div className="flex items-center justify-between gap-2">
                  <span className="text-sm font-medium">
                    {member.name}
                    {member.id === snapshot?.endpointId ? "（你）" : ""}
                  </span>
                  {member.id === room.share?.publisherId && (
                    <Badge variant="secondary">分享中</Badge>
                  )}
                </div>
              </li>
            ))}
          </ul>
        </Surface>
      </div>
    </>
  );
}
