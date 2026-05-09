// Deterministic fallback and sequencer maintenance for In C.
//
// The LLM-backed `conductor` agent is the primary conductor. This script keeps
// each cell_sequencer's `steps` aligned to the active cell length, and only
// makes progression decisions when the conductor is disabled or has not
// completed a request recently.

let cfg = null;
let cellLengths = null;
let lastCellApplied = null;
let lastConductorRequestCount = 0;
let ticksSinceConductor = 0;
let lastConductorHealthy = null;

function readOwnConfig() {
  const modules = graph.listModules();
  const me = modules.find((m) => m.id === graph.moduleId);
  return (me && me.config) || {};
}

function init() {
  const c = readOwnConfig();
  cfg = {
    conductorId: c.conductor_id || "conductor",
    sequencers: Array.isArray(c.sequencer_ids) ? c.sequencer_ids.slice() : [],
    mixerId: c.mixer_id || "mixer",
    reverbId: c.reverb_id || "reverb",
    minLoops:
      typeof c.min_loops_before_advance === "number"
        ? c.min_loops_before_advance
        : 4,
    maxAhead:
      typeof c.max_cells_ahead_of_slowest === "number"
        ? c.max_cells_ahead_of_slowest
        : 2,
    advanceProb:
      typeof c.advance_probability === "number" ? c.advance_probability : 0.15,
    timeoutTicks:
      typeof c.conductor_timeout_ticks === "number"
        ? c.conductor_timeout_ticks
        : 24,
    lastCellHoldLoops:
      typeof c.last_cell_hold_loops === "number" ? c.last_cell_hold_loops : 4,
  };
  if (cfg.sequencers.length === 0) {
    throw new Error("conductor_fallback: missing sequencer_ids");
  }

  const sequencesJson = graph.getControl(cfg.sequencers[0], "sequences_json");
  try {
    const cells = JSON.parse(sequencesJson);
    cellLengths = Array.isArray(cells)
      ? cells.map((cell) => (Array.isArray(cell) ? cell.length : 0))
      : [];
  } catch (_err) {
    cellLengths = [];
  }
  lastCellApplied = cfg.sequencers.map(() => -1);
  lastConductorRequestCount = readNumber(cfg.conductorId, "request_count");
  ticksSinceConductor = 0;
}

function readNumber(moduleId, control) {
  const value = graph.getControl(moduleId, control);
  return typeof value === "number" ? value : 0;
}

function readBool(moduleId, control) {
  return graph.getControl(moduleId, control) === true;
}

function pulseAdvance(sequencerId) {
  graph.setControl(sequencerId, "advance", 1);
  graph.setControl(sequencerId, "advance", 0);
}

function syncSteps(index, cell) {
  if (cell === lastCellApplied[index]) return;
  if (cellLengths && cell >= 0 && cell < cellLengths.length) {
    const len = cellLengths[cell];
    if (len > 0) {
      graph.setControl(cfg.sequencers[index], "steps", len);
    }
  }
  lastCellApplied[index] = cell;
}

function conductorIsHealthy() {
  let enabled = false;
  try {
    enabled = readBool(cfg.conductorId, "enabled");
  } catch (_err) {
    return { healthy: false, reason: "conductor module not found" };
  }
  if (!enabled) return { healthy: false, reason: "conductor disabled" };

  const requestCount = readNumber(cfg.conductorId, "request_count");
  if (requestCount > lastConductorRequestCount) {
    lastConductorRequestCount = requestCount;
    ticksSinceConductor = 0;
  } else {
    ticksSinceConductor += 1;
  }

  const status = graph.getControl(cfg.conductorId, "status");
  const lastError = graph.getControl(cfg.conductorId, "last_error");

  const activeRequest = status === "requesting" || status === "building_context";
  if (requestCount === 0 && !activeRequest) {
    return { healthy: false, reason: "no requests completed (status: " + status + (lastError ? ", error: " + lastError : "") + ")" };
  }
  if (ticksSinceConductor > cfg.timeoutTicks && !activeRequest) {
    return { healthy: false, reason: "conductor timed out (" + ticksSinceConductor + " ticks since last request)" };
  }
  if (status === "error") {
    return { healthy: false, reason: "conductor in error state: " + lastError };
  }
  if (lastError !== "") {
    return { healthy: false, reason: "conductor has error: " + lastError };
  }
  return { healthy: true, reason: null };
}

function tick() {
  if (!cfg) return;

  const cells = [];
  const loops = [];
  let slowest = 9999;
  let allAtLast = true;
  let totalCells = 0;

  for (let i = 0; i < cfg.sequencers.length; i++) {
    const id = cfg.sequencers[i];
    const cell = readNumber(id, "current_cell");
    const loopCount = readNumber(id, "loop_count");
    const total = readNumber(id, "total_cells");
    cells.push(cell);
    loops.push(loopCount);
    syncSteps(i, cell);
    if (cell < slowest) slowest = cell;
    if (total > totalCells) totalCells = total;
    if (cell < total - 1) allAtLast = false;
  }

  const result = conductorIsHealthy();
  if (result.healthy !== lastConductorHealthy) {
    if (result.healthy) {
      console.log("[in_c] conductor agent is driving");
    } else {
      console.log("[in_c] script fallback is driving: " + result.reason);
    }
    lastConductorHealthy = result.healthy;
  }
  if (result.healthy) return;

  for (let i = 0; i < cfg.sequencers.length; i++) {
    const id = cfg.sequencers[i];
    const cell = cells[i];
    const loopCount = loops[i];
    if (loopCount < cfg.minLoops) continue;

    if (cell >= totalCells - 1) {
      if (allAtLast && loopCount >= cfg.lastCellHoldLoops) {
        graph.setControl(id, "selected_sequence", 0);
      }
      continue;
    }

    if (cell - slowest >= cfg.maxAhead) continue;
    if (Math.random() < cfg.advanceProb) {
      pulseAdvance(id);
    }
  }
}

function reset() {
  cfg = null;
  cellLengths = null;
  lastCellApplied = null;
  lastConductorRequestCount = 0;
  ticksSinceConductor = 0;
  lastConductorHealthy = null;
}
