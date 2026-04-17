import { FugueEngine } from "./fugue.js";

function parseJson(method, value) {
  try {
    return JSON.parse(value);
  } catch (error) {
    throw new Error(`${method} returned invalid JSON: ${error}`);
  }
}

function toConfigJson(config) {
  if (config === undefined || config === null) {
    return undefined;
  }
  return JSON.stringify(config);
}

export { FugueEngine };

export class WasmCodeHost {
  constructor(engine) {
    this.engine = engine;
    this.sessions = new Map();
  }

  static create(sampleRate) {
    return new WasmCodeHost(new FugueEngine(sampleRate));
  }

  sampleRate() {
    return this.engine.sampleRate();
  }

  loadInvention(json) {
    this.stop();
    this.engine.loadInvention(json);
    this.startCodeModules();
  }

  reset() {
    this.stop();
    this.engine.reset();
    this.startCodeModules();
  }

  status() {
    return parseJson("status", this.engine.status());
  }

  listModules() {
    return parseJson("listModules", this.engine.listModules());
  }

  listConnections() {
    return parseJson("listConnections", this.engine.listConnections());
  }

  listControls(moduleId) {
    return parseJson("listControls", this.engine.listControls(moduleId));
  }

  listCodeModules() {
    return parseJson("listCodeModules", this.engine.listCodeModules());
  }

  getCodeModuleConfig(moduleId) {
    return parseJson("getCodeModuleConfig", this.engine.getCodeModuleConfig(moduleId));
  }

  getControl(moduleId, key) {
    return parseJson("getControl", this.engine.getControl(moduleId, key));
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

  addModule(id, moduleType, config) {
    this.engine.addModule(id, moduleType, toConfigJson(config));
    if (moduleType === "code") {
      this.startModuleById(id);
    }
  }

  removeModule(id) {
    this.stopModule(id);
    this.engine.removeModule(id);
  }

  connect(from, fromPort, to, toPort) {
    this.engine.connect(from, fromPort, to, toPort);
  }

  disconnect(from, fromPort, to, toPort) {
    this.engine.disconnect(from, fromPort, to, toPort);
  }

  renderInterleaved(frameCount) {
    return this.engine.renderInterleaved(frameCount);
  }

  stop() {
    for (const moduleId of Array.from(this.sessions.keys())) {
      this.stopModule(moduleId);
    }
  }

  dispose() {
    this.stop();
  }

  startCodeModules() {
    for (const module of this.listCodeModules()) {
      this.startModule(module);
    }
  }

  startModuleById(moduleId) {
    this.startModule(this.getCodeModuleConfig(moduleId));
  }

  startModule(moduleConfig) {
    if (!moduleConfig || !moduleConfig.enabled) {
      return;
    }

    this.stopModule(moduleConfig.id);

    const graph = this.createGraphApi(moduleConfig.id);
    const escapedScript = JSON.stringify(moduleConfig.script);
    const escapedEntrypoint = JSON.stringify(moduleConfig.entrypoint);
    const factory = new Function(
      "graph",
      "fetch",
      `const __fugue_legacy = Object.create(null);\nconst globalThis = __fugue_legacy;\nconst __fugue_result = eval(${escapedScript});\nconst __fugue_resolve = (name) => {\n  if (typeof __fugue_result === 'object' && __fugue_result !== null && typeof __fugue_result[name] === 'function') {\n    return __fugue_result[name];\n  }\n  try {\n    const candidate = eval(name);\n    if (typeof candidate === 'function') {\n      return candidate;\n    }\n  } catch (_error) {}\n  if (typeof __fugue_legacy[name] === 'function') {\n    return __fugue_legacy[name];\n  }\n  return undefined;\n};\nreturn {\n  init: __fugue_resolve('init'),\n  reset: __fugue_resolve('reset'),\n  tick: __fugue_resolve('tick'),\n  entrypoint: __fugue_resolve(${escapedEntrypoint}),\n};`
    );

    let hooks;
    try {
      this.engine.setCodeModuleError(moduleConfig.id, "");
      this.engine.setCodeModuleStatus(moduleConfig.id, "starting");
      hooks = factory(graph, (...args) => fetch(...args));
    } catch (error) {
      this.handleModuleError(moduleConfig.id, error);
      return;
    }

    const session = {
      moduleId: moduleConfig.id,
      hooks,
      timer: null,
    };
    this.sessions.set(moduleConfig.id, session);

    const startupHook = hooks.entrypoint || hooks.init;
    if (startupHook) {
      try {
        startupHook();
      } catch (error) {
        this.sessions.delete(moduleConfig.id);
        this.handleModuleError(moduleConfig.id, error);
        return;
      }
    }

    this.engine.setCodeModuleStatus(moduleConfig.id, "running");
    if (moduleConfig.tick_hz > 0) {
      session.timer = setInterval(() => {
        const latest = this.getCodeModuleConfig(moduleConfig.id);
        if (!latest.enabled || !session.hooks.tick) {
          return;
        }
        try {
          session.hooks.tick();
        } catch (error) {
          this.handleModuleError(moduleConfig.id, error);
        }
      }, 1000 / moduleConfig.tick_hz);
    }
  }

  stopModule(moduleId) {
    const session = this.sessions.get(moduleId);
    if (!session) {
      return;
    }
    if (session.timer !== null) {
      clearInterval(session.timer);
    }
    if (session.hooks.reset) {
      try {
        session.hooks.reset();
      } catch (error) {
        this.handleModuleError(moduleId, error);
      }
    }
    this.sessions.delete(moduleId);
  }

  handleModuleError(moduleId, error) {
    const message = error instanceof Error ? error.message : String(error);
    const session = this.sessions.get(moduleId);
    if (session && session.timer !== null) {
      clearInterval(session.timer);
    }
    this.sessions.delete(moduleId);
    this.engine.setCodeModuleStatus(moduleId, "error");
    this.engine.setCodeModuleError(moduleId, message);
  }

  createGraphApi(ownerModuleId) {
    return {
      moduleId: ownerModuleId,
      status: () => this.status(),
      listModules: () => this.listModules(),
      listConnections: () => this.listConnections(),
      listControls: (moduleId) => this.listControls(moduleId),
      getControl: (moduleId, key) => this.getControl(moduleId, key),
      setControl: (moduleId, key, value) => this.setControl(moduleId, key, value),
      addModule: (id, moduleType, config) => this.addModule(id, moduleType, config),
      removeModule: (id) => this.removeModule(id),
      connect: (from, fromPort, to, toPort) => this.connect(from, fromPort, to, toPort),
      disconnect: (from, fromPort, to, toPort) =>
        this.disconnect(from, fromPort, to, toPort),
      fetch: (...args) => fetch(...args),
    };
  }
}
