import { Button } from "@cloudflare/kumo/components/button";
import { Input } from "@cloudflare/kumo/components/input";
import { Select } from "@cloudflare/kumo/components/select";
import { Dialog } from "@cloudflare/kumo/components/dialog";
import { encodingBudget } from "../media/quality";
import type { SessionState } from "../session/useSession";

export function QualityDialog({ session }: { session: SessionState }) {
  const {
    qualityError,
    mode,
    setMode,
    height,
    setHeight,
    fps,
    setFps,
    bitrate,
    setBitrate,
    available,
    ownCapture,
    room,
    snapshot,
    busy,
    media,
    setQualityError,
    perform,
    updateQuality,
  } = session;
  return (
    <Dialog
      size="lg"
      className="grid max-h-[90dvh] gap-5 overflow-y-auto px-5 py-4"
    >
      <div className="grid gap-1.5">
        <Dialog.Title className="text-lg font-semibold text-kumo-strong">
          分享画质
        </Dialog.Title>
        <Dialog.Description className="sr-only">
          调整当前分享的画质模式、分辨率、帧率和码率上限。
        </Dialog.Description>
      </div>
      {qualityError && (
        <p role="alert" className="text-sm text-kumo-danger">
          {qualityError}
        </p>
      )}
      <div className="grid gap-4 sm:grid-cols-2">
        <Select
          label="画质模式"
          className="w-full"
          size="sm"
          value={mode}
          renderValue={(value) => (value === "auto" ? "自动" : "原画")}
          onValueChange={(value) =>
            setMode(value === "auto" ? "auto" : "original")
          }
          disabled={!available || !ownCapture}
        >
          <Select.Option value="original">原画</Select.Option>
          <Select.Option value="auto">自动</Select.Option>
        </Select>
        <Select
          label={mode === "auto" ? "分辨率上限" : "分辨率"}
          className="w-full"
          size="sm"
          value={height}
          renderValue={(value) => `${value}p`}
          onValueChange={(value) => setHeight(value ?? "1080")}
          disabled={!available || !ownCapture}
        >
          {[480, 720, 1080, 1440, 2160].map((value) => (
            <Select.Option key={value} value={String(value)}>
              {value}p
            </Select.Option>
          ))}
        </Select>
        <Input
          label={mode === "auto" ? "帧率上限（fps）" : "帧率（fps）"}
          type="number"
          min={15}
          max={60}
          step={1}
          value={fps}
          onChange={(event) => setFps(event.target.value)}
          disabled={!available || !ownCapture}
        />
        <Input
          label="码率上限（Mbps）"
          type="number"
          min={0.5}
          max={30}
          step={0.5}
          value={bitrate}
          onChange={(event) => setBitrate(event.target.value)}
          disabled={!available || !ownCapture}
        />
      </div>
      {room?.share?.profile.mode === "auto" &&
        snapshot?.networkBitrate != null && (
          <p className="text-sm text-kumo-subtle">
            实时码率上限{" "}
            {(
              encodingBudget(
                room.share.profile,
                room.share.audio,
                snapshot.networkBitrate,
              ).video / 1_000_000
            ).toFixed(2)}{" "}
            Mbps
          </p>
        )}
      <div className="flex justify-end gap-2">
        <Dialog.Close render={<Button disabled={busy}>取消</Button>} />
        <Button
          variant="primary"
          disabled={!available || !ownCapture || !media.capture?.quality}
          loading={busy}
          onClick={() => {
            setQualityError(null);
            void perform(updateQuality, setQualityError);
          }}
        >
          应用画质
        </Button>
      </div>
    </Dialog>
  );
}
