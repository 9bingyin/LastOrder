import { Button } from "@cloudflare/kumo/components/button";
import { CheckIcon, CopyIcon, SignOutIcon } from "@phosphor-icons/react";
import { useState } from "react";
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
  const [copied, setCopied] = useState<string | null>(null);
  if (!room) return null;
  const members = Object.values(room.members);
  const copy = (key: string, value: string) =>
    void perform(async () => {
      await navigator.clipboard.writeText(value);
      setCopied(key);
      setTimeout(() => setCopied((current) => (current === key ? null : current)), 1500);
    });
  const invites = [
    ["code", "复制加入码", snapshot?.joinCode],
    ["ticket", "复制 Ticket", snapshot?.joinTicket],
  ] as const;
  return (
    <div className="grid gap-5">
      <div className="flex flex-wrap items-start justify-between gap-4">
        <div className="grid min-w-0 gap-2">
          <h2 className="flex items-baseline gap-2 text-xl font-semibold text-kumo-strong">
            房间
            <span className="text-sm font-normal text-kumo-subtle tabular-nums">
              {members.length} 人
            </span>
          </h2>
          <ul aria-label="房间成员" className="flex flex-wrap gap-1.5">
            {members.map((member) => {
              const sharing = member.id === room.share?.publisherId;
              return (
                <li
                  key={member.id}
                  className={`inline-flex items-center gap-1.5 rounded-full px-2.5 py-0.5 text-sm ring ${sharing ? "bg-kumo-danger-tint text-kumo-danger ring-kumo-danger/20" : "bg-kumo-control/50 ring-kumo-line"}`}
                >
                  {sharing && (
                    <span
                      aria-hidden="true"
                      className="size-1.5 animate-pulse rounded-full bg-kumo-danger"
                    />
                  )}
                  {member.name}
                  {member.id === snapshot?.endpointId && (
                    <span className="text-kumo-subtle">（你）</span>
                  )}
                  {sharing && <span className="sr-only">分享中</span>}
                </li>
              );
            })}
          </ul>
        </div>
        <div className="flex flex-wrap gap-2">
          {invites.map(
            ([key, label, value]) =>
              value && (
                <Button
                  key={key}
                  icon={copied === key ? CheckIcon : CopyIcon}
                  disabled={!available}
                  onClick={() => copy(key, value)}
                >
                  {copied === key ? "已复制" : label}
                </Button>
              ),
          )}
          <Button
            variant="secondary-destructive"
            icon={SignOutIcon}
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
      <Player session={session} />
    </div>
  );
}
