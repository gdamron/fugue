import init, { FugueEngine } from "./fugue.js";

class FugueAudioWorkletProcessor extends AudioWorkletProcessor {
  constructor(options) {
    super();

    this.engine = null;
    this.ready = false;
    this.playing = false;
    this.renderBuffer = new Float32Array(0);

    this.port.onmessage = (event) => {
      this.handleMessage(event.data);
    };

    this.initialize(options.processorOptions || {});
  }

  async initialize(options) {
    try {
      await init(options.wasmUrl);
      this.engine = new FugueEngine(options.sampleRate || sampleRate);
      this.ready = true;
      this.port.postMessage({ type: "ready" });
    } catch (error) {
      this.postError(error);
    }
  }

  handleMessage(message) {
    if (!message || typeof message !== "object") {
      return;
    }

    if (message.type === "play") {
      this.playing = true;
      this.port.postMessage({ type: "state", state: "playing" });
      return;
    }

    if (message.type === "stop") {
      this.playing = false;
      this.port.postMessage({ type: "state", state: "stopped" });
      return;
    }

    if (message.type !== "request") {
      return;
    }

    try {
      const result = this.handleRequest(message.method, message.args || []);
      this.port.postMessage({ type: "response", id: message.id, result });
    } catch (error) {
      this.port.postMessage({
        type: "response",
        id: message.id,
        error: error instanceof Error ? error.message : String(error),
      });
    }
  }

  handleRequest(method, args) {
    if (!this.ready || !this.engine) {
      throw new Error("Fugue engine is not ready");
    }

    switch (method) {
      case "loadInvention":
        this.engine.loadInvention(args[0]);
        return null;
      case "reset":
        this.engine.reset();
        return null;
      case "status":
        return this.engine.status();
      case "listModules":
        return this.engine.listModules();
      case "listConnections":
        return this.engine.listConnections();
      case "listControls":
        return this.engine.listControls(args[0]);
      case "getControl":
        return this.engine.getControl(args[0], args[1]);
      case "setControl":
        this.setControl(args[0], args[1], args[2]);
        return null;
      default:
        throw new Error(`Unknown Fugue player method: ${method}`);
    }
  }

  setControl(moduleId, key, value) {
    switch (typeof value) {
      case "number":
        this.engine.setControlNumber(moduleId, key, value);
        return;
      case "boolean":
        this.engine.setControlBool(moduleId, key, value);
        return;
      case "string":
        this.engine.setControlString(moduleId, key, value);
        return;
      default:
        throw new TypeError(`Unsupported control value type: ${typeof value}`);
    }
  }

  process(_inputs, outputs) {
    const output = outputs[0];
    const left = output && output[0];
    const right = output && output[1];
    if (!left || !right) {
      return true;
    }

    if (!this.playing || !this.ready || !this.engine) {
      left.fill(0);
      right.fill(0);
      return true;
    }

    const frames = left.length;
    const sampleCount = frames * 2;
    if (this.renderBuffer.length !== sampleCount) {
      this.renderBuffer = new Float32Array(sampleCount);
    }

    try {
      this.engine.renderInterleavedInto(this.renderBuffer);
      for (let frame = 0, sample = 0; frame < frames; frame += 1, sample += 2) {
        left[frame] = this.renderBuffer[sample];
        right[frame] = this.renderBuffer[sample + 1];
      }
    } catch (error) {
      this.playing = false;
      left.fill(0);
      right.fill(0);
      this.postError(error);
    }

    return true;
  }

  postError(error) {
    this.port.postMessage({
      type: "error",
      error: error instanceof Error ? error.message : String(error),
    });
  }
}

registerProcessor("fugue-audio-worklet", FugueAudioWorkletProcessor);
