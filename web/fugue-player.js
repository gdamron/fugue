function parseJson(method, value) {
  try {
    return JSON.parse(value);
  } catch (error) {
    throw new Error(`${method} returned invalid JSON: ${error}`);
  }
}

function defaultWorkletUrl() {
  return new URL("./fugue-audio-worklet.js", import.meta.url);
}

export class FuguePlayer {
  constructor(context, node, options = {}) {
    this.context = context;
    this.node = node;
    this.options = options;
    this.requests = new Map();
    this.nextRequestId = 1;
    this.connected = false;
    this.disposed = false;
    this.state = "stopped";

    this.node.port.onmessage = (event) => {
      this.handleMessage(event.data);
    };
  }

  static async create(options = {}) {
    const context =
      options.audioContext ||
      new AudioContext(
        options.sampleRate === undefined ? undefined : { sampleRate: options.sampleRate }
      );
    const workletUrl = options.workletUrl || defaultWorkletUrl();

    await context.audioWorklet.addModule(workletUrl);
    const node = new AudioWorkletNode(context, "fugue-audio-worklet", {
      numberOfInputs: 0,
      numberOfOutputs: 1,
      outputChannelCount: [2],
      processorOptions: {
        sampleRate: context.sampleRate,
        wasmUrl: options.wasmUrl,
      },
    });

    const player = new FuguePlayer(context, node, options);
    await player.waitUntilReady();
    return player;
  }

  waitUntilReady() {
    if (this.readyPromise) {
      return this.readyPromise;
    }

    this.readyPromise = new Promise((resolve, reject) => {
      this.resolveReady = resolve;
      this.rejectReady = reject;
    });
    return this.readyPromise;
  }

  async loadInvention(json) {
    await this.request("loadInvention", [json]);
  }

  async play() {
    this.assertActive();
    if (!this.connected) {
      this.node.connect(this.context.destination);
      this.connected = true;
    }
    await this.context.resume();
    this.node.port.postMessage({ type: "play" });
    this.state = "playing";
    this.options.onStateChange?.(this.state);
  }

  stop() {
    this.assertActive();
    this.node.port.postMessage({ type: "stop" });
    this.state = "stopped";
    this.options.onStateChange?.(this.state);
  }

  async reset() {
    await this.request("reset");
  }

  async status() {
    return parseJson("status", await this.request("status"));
  }

  async listModules() {
    return parseJson("listModules", await this.request("listModules"));
  }

  async listConnections() {
    return parseJson("listConnections", await this.request("listConnections"));
  }

  async listControls(moduleId) {
    return parseJson("listControls", await this.request("listControls", [moduleId]));
  }

  async getControl(moduleId, key) {
    return parseJson("getControl", await this.request("getControl", [moduleId, key]));
  }

  async setControl(moduleId, key, value) {
    await this.request("setControl", [moduleId, key, value]);
  }

  async dispose() {
    if (this.disposed) {
      return;
    }
    this.stop();
    this.node.port.close();
    if (this.connected) {
      this.node.disconnect();
      this.connected = false;
    }
    if (!this.options.audioContext) {
      await this.context.close();
    }
    this.disposed = true;
  }

  request(method, args = []) {
    this.assertActive();
    const id = this.nextRequestId;
    this.nextRequestId += 1;

    return new Promise((resolve, reject) => {
      this.requests.set(id, { resolve, reject });
      this.node.port.postMessage({ type: "request", id, method, args });
    });
  }

  handleMessage(message) {
    if (!message || typeof message !== "object") {
      return;
    }

    if (message.type === "ready") {
      this.resolveReady?.();
      return;
    }

    if (message.type === "state") {
      this.state = message.state;
      this.options.onStateChange?.(message.state);
      return;
    }

    if (message.type === "error") {
      const error = new Error(message.error);
      this.rejectReady?.(error);
      this.options.onError?.(error);
      return;
    }

    if (message.type !== "response") {
      return;
    }

    const request = this.requests.get(message.id);
    if (!request) {
      return;
    }
    this.requests.delete(message.id);

    if (message.error) {
      request.reject(new Error(message.error));
    } else {
      request.resolve(message.result);
    }
  }

  assertActive() {
    if (this.disposed) {
      throw new Error("FuguePlayer has been disposed");
    }
  }
}
