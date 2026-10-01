import { z } from "zod";
import { errorMessage } from "../api";
import {
  debugEvents,
  diagnosticsSchema,
  mediaDiagnostics,
  redactDebug,
} from "./diagnostics";
import type { DebugMedia, Diagnostics } from "./diagnostics";

const sampleSchema = z.object({
  time: z.string(),
  backend: diagnosticsSchema.nullable(),
  browser: z.unknown(),
  media: z.array(z.unknown()),
  events: z.unknown(),
  failure: z.string().nullable(),
});
export type Sample = z.infer<typeof sampleSchema>;

export function startSampling(
  token: string,
  readMedia: () => DebugMedia[],
  readBrowser: () => unknown,
  publish: (sample: Sample) => void,
) {
  const controller = new AbortController();
  let disposed = false;
  let timer: ReturnType<typeof setTimeout> | undefined;
  const poll = async () => {
    let backend: Diagnostics | null = null;
    let failure: string | null = null;
    const media = readMedia();
    const [native] = await Promise.all([
      Promise.all(
        media.map(async (resource) => {
          try {
            return await mediaDiagnostics(resource);
          } catch (error) {
            return {
              id: resource.id,
              kind: resource.kind,
              error: errorMessage(error),
            };
          }
        }),
      ),
      (async () => {
        try {
          const response = await fetch("/api/v1/debug", {
            headers: { Authorization: `Bearer ${token}` },
            signal: AbortSignal.any([
              controller.signal,
              AbortSignal.timeout(8000),
            ]),
          });
          if (!response.ok)
            throw new Error(
              `诊断请求失败（${response.status}），请确认 CLI 已更新`,
            );
          backend = diagnosticsSchema.parse(await response.json());
        } catch (error) {
          failure = errorMessage(error);
        }
      })(),
    ]);
    if (disposed) return;
    const ids = new Set(readMedia().map((resource) => resource.id));
    const sample = {
      time: new Date().toISOString(),
      backend,
      browser: readBrowser(),
      media: native.filter((resource) => ids.has(resource.id)),
      events: debugEvents(),
      failure,
    };
    publish(sampleSchema.parse(redactDebug(sample, [token])));
    if (!disposed) timer = setTimeout(() => void poll(), 1000);
  };
  void poll();
  return () => {
    disposed = true;
    controller.abort();
    clearTimeout(timer);
  };
}
