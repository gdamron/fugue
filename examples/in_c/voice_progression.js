// Per-voice progression script for In C.
//
// Attaches to a `code` module that orchestrates a single voice. Each tick the
// script reads its own cell_sequencer state and the peer voices' progress, then
// decides whether to pulse the cell_sequencer's `advance` control to move to
// the next cell. The script also keeps the sequencer's `steps` aligned with the
// length of the active cell so cells of different lengths loop cleanly.
//
// Required code-module config (read once in init from listModules):
//   sequencer_id: string                  // id of this voice's cell_sequencer
//   peer_voice_ids: string[]              // peer voices' cell_sequencer ids
//   min_loops_before_advance: number      // default 4
//   max_cells_ahead_of_slowest: number    // default 2
//   advance_probability: number           // [0, 1], default 0.15
//   last_cell_behavior: "hold" | "loop"   // default "hold"
//
// Required cell_sequencer controls (per FUG-86 contract):
//   loop_count   : number  — completed loops of the current cell
//   current_cell : number  — active cell index (0-based)
//   advance      : trigger — rising edge advances to the next cell
//   total_cells  : number  — count of cells in the sequence bank
//
// Hot-path note: `tick` makes no allocations beyond the integers it reads
// back; peer ids, tuning, and per-cell step counts are cached in init.

let cfg = null;
let cellLengths = null;
let lastCellApplied = -1;

function readOwnConfig() {
  const modules = graph.listModules();
  const me = modules.find((m) => m.id === graph.moduleId);
  return (me && me.config) || {};
}

function init() {
  const c = readOwnConfig();
  cfg = {
    sequencerId: c.sequencer_id,
    peers: Array.isArray(c.peer_voice_ids) ? c.peer_voice_ids.slice() : [],
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
    lastCellBehavior: c.last_cell_behavior === "loop" ? "loop" : "hold",
  };
  if (!cfg.sequencerId) {
    throw new Error(
      `voice_progression: code module '${graph.moduleId}' is missing 'sequencer_id' in config`,
    );
  }

  const sequencesJson = graph.getControl(cfg.sequencerId, "sequences_json");
  try {
    const cells = JSON.parse(sequencesJson);
    cellLengths = Array.isArray(cells)
      ? cells.map((cell) => (Array.isArray(cell) ? cell.length : 0))
      : [];
  } catch (_err) {
    cellLengths = [];
  }
  lastCellApplied = -1;
}

function pulseAdvance() {
  graph.setControl(cfg.sequencerId, "advance", 1);
  graph.setControl(cfg.sequencerId, "advance", 0);
}

function syncStepsForCell(cell) {
  if (cell === lastCellApplied) return;
  if (cellLengths && cell >= 0 && cell < cellLengths.length) {
    const len = cellLengths[cell];
    if (len > 0) {
      graph.setControl(cfg.sequencerId, "steps", len);
    }
  }
  lastCellApplied = cell;
}

function tick() {
  if (!cfg) return;

  const myCell = graph.getControl(cfg.sequencerId, "current_cell");
  syncStepsForCell(myCell);

  const myLoops = graph.getControl(cfg.sequencerId, "loop_count");
  if (myLoops < cfg.minLoops) return;

  const totalCells = graph.getControl(cfg.sequencerId, "total_cells");

  if (myCell >= totalCells - 1) {
    if (cfg.lastCellBehavior === "loop" && Math.random() < cfg.advanceProb) {
      // Wrap to cell 0 by selecting it directly.
      graph.setControl(cfg.sequencerId, "selected_sequence", 0);
    }
    return;
  }

  let slowest = myCell;
  for (let i = 0; i < cfg.peers.length; i++) {
    const peerCell = graph.getControl(cfg.peers[i], "current_cell");
    if (peerCell < slowest) slowest = peerCell;
  }
  if (myCell - slowest >= cfg.maxAhead) return;

  if (Math.random() < cfg.advanceProb) {
    pulseAdvance();
  }
}

function reset() {
  cfg = null;
  cellLengths = null;
  lastCellApplied = -1;
}
