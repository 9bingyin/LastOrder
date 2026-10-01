export function deferred<T>() {
  let resolve!: (value: T | PromiseLike<T>) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((done, fail) => {
    resolve = done;
    reject = fail;
  });
  return { promise, resolve, reject };
}

export function fakeMedia() {
  let live = true;
  let closed = false;
  let parameters: RTCRtpSendParameters = {
    transactionId: "test",
    codecs: [],
    headerExtensions: [],
    rtcp: {},
    encodings: [{}],
  };
  let constraints: MediaTrackConstraints = {};
  const track = new (class extends EventTarget {
    id = "video";
    kind = "video";
    contentHint = "";
    label = "test";
    enabled = true;
    muted = false;
    get readyState() {
      return live ? "live" : "ended";
    }
    stop() {
      live = false;
    }
    getSettings() {
      return { width: 1920, height: 1080 };
    }
    getConstraints() {
      return constraints;
    }
    getCapabilities() {
      return {};
    }
    async applyConstraints(next: MediaTrackConstraints) {
      constraints = next;
    }
  })();
  // Only the browser API members consumed by these owners are implemented.
  const browserTrack = track as unknown as MediaStreamTrack;
  const stream = {
    id: "stream",
    active: true,
    getTracks: () => [browserTrack],
    getVideoTracks: () => [browserTrack],
    getAudioTracks: () => [],
  } as unknown as MediaStream;
  const sender = {
    track: browserTrack,
    getParameters: () => structuredClone(parameters),
    async setParameters(next: RTCRtpSendParameters) {
      parameters = structuredClone(next);
    },
  };
  const peer = {
    getSenders: () => [sender as unknown as RTCRtpSender],
    async getStats(): Promise<RTCStatsReport> {
      return new Map();
    },
    getConfiguration: () => ({}),
    getReceivers: () => [],
    getTransceivers: () => [],
    close() {
      closed = true;
    },
    get connectionState() {
      return closed ? "closed" : "connected";
    },
  };
  const pc = peer as unknown as RTCPeerConnection;
  return {
    track,
    stream,
    sender,
    peer,
    pc,
    get live() {
      return live;
    },
    get closed() {
      return closed;
    },
    parameters: () => parameters,
  };
}
