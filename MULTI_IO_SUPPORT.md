# Multiple Inputs/Outputs Support - Implementation Summary

## What Was Requested

The user updated `dorian_melody.json` to have:
- **One clock** feeding **two melody generators**
- **Two independent voice chains**  
- **Both voices** feeding into **one DAC**

This created an error: "Module clock has multiple outputs"

The request was to add support for:
- Multiple outputs from one module
- Multiple inputs to one module
- Optionally, a Mixer node to keep things clean

## What Was Implemented

### 1. Mixer Module (`src/synthesis.rs`)

Created a new `Mixer` module that can combine multiple audio signals:

```rust
pub struct Mixer {
    num_inputs: usize,
    gain: f32,  // Auto-adjusted to prevent clipping
}
```

Features:
- Automatic gain compensation based on number of inputs (`1.0 / sqrt(n)`)
- Can be configured with custom gain
- Implements the `Module` trait

### 2. Complete Builder Rewrite (`src/builder.rs`)

**Old Approach:**
- Validated that patches formed a single linear chain
- Rejected multiple outputs with error message
- Used concrete types with `ConnectedProcessor`

**New Approach:**
- Supports arbitrary directed acyclic graphs (DAGs)
- Validates:
  - No cycles
  - Exactly one source (clock)
  - Exactly one sink (DAC)
  - All connections reference existing modules
- Builds parallel signal paths automatically
- Automatically mixes multiple inputs to DAC

**Key Implementation Details:**

1. **Graph Validation**
   - Cycle detection using DFS with visited/stack sets
   - Connection validation (all IDs must exist)
   - Source/sink validation

2. **Parallel Path Building**
   - Finds all paths from clock to DAC
   - Builds independent voice chains for each path
   - Each voice chain: `Clock -> Melody -> Voice -> DAC`

3. **Automatic Mixing**
   - When multiple voices feed the DAC, they're automatically mixed
   - Gain compensation: `gain = 1.0 / sqrt(num_voices)`
   - No explicit mixer node needed in the JSON

4. **Thread Safety**
   - Changed from `RefCell` to `Mutex` for thread-safe interior mutability
   - All modules wrapped in `Arc<Mutex<T>>` for shared ownership
   - Proper lock handling to avoid deadlocks

### 3. Internal Architecture

**`SignalGraph` Structure:**
```rust
struct SignalGraph {
    clock: Arc<Mutex<Clock>>,
    voices: Vec<VoiceChain>,
    tempo: Tempo,
    melody_params_list: Vec<MelodyParams>,
}
```

**`VoiceChain` Structure:**
```rust
struct VoiceChain {
    melody: Arc<Mutex<MelodyGenerator>>,
    voice: Arc<Mutex<Voice>>,
}
```

**Processing Flow:**
1. Lock and process clock -> get `ClockSignal`
2. For each voice chain:
   - Process melody with clock signal -> get `NoteSignal`
   - Process voice with note signal -> get `Audio`
3. Mix all audio samples with gain compensation
4. Output mixed audio

### 4. Updated Documentation

Updated `DECLARATIVE.md` to document:
- Parallel signal paths feature
- Example dual-voice patch
- Multiple connection rules
- Automatic mixing behavior

## Example: Your dorian_melody.json

Your patch now works perfectly:

```json
{
  "connections": [
    {"from": "clock", "to": "melody"},      // Path 1: clock -> melody
    {"from": "clock", "to": "melody_2"},    // Path 2: clock -> melody_2
    {"from": "melody", "to": "voice"},      // Path 1: melody -> voice
    {"from": "melody_2", "to": "voice_2"},  // Path 2: melody_2 -> voice_2
    {"from": "voice", "to": "dac"},         // Path 1: voice -> dac
    {"from": "voice_2", "to": "dac"}        // Path 2: voice_2 -> dac
  ]
}
```

This creates:
- **2 independent synthesis paths**
- **Shared clock timing** (both melodies sync to same tempo)
- **Automatic mixing** at the DAC (both voices audible)
- **Gain compensation** to prevent clipping

## Supported Patterns

### One-to-Many (Fan-out)
```
   clock
   /   \
melody1 melody2
```

### Many-to-One (Fan-in, Auto-mix)
```
voice1 \
        dac
voice2 /
```

### Parallel Chains
```
        -> melody1 -> voice1 \
clock                          dac
        -> melody2 -> voice2 /
```

## Runtime Control

The `RunningPatch` provides access to all melody parameters:

```rust
// Get first melody params (backward compatible)
running.melody_params() -> &MelodyParams

// Get all melody params (new)
running.all_melody_params() -> &[MelodyParams]
```

This allows runtime control of:
- Individual melody parameters
- Shared tempo (affects all chains)
- Per-voice oscillator types
- Note durations, weights, scales, etc.

## Technical Highlights

1. **No Explicit Mixer Node Required**
   - System automatically mixes at DAC
   - Transparent to user in simple cases
   - Mixer module available for future explicit mixing

2. **Thread-Safe Design**
   - All modules use `Arc<Mutex<T>>`
   - Safe for audio thread and UI thread access
   - Proper lock ordering prevents deadlocks

3. **Type-Safe Graph**
   - Validates signal types at build time
   - Ensures clock -> melody -> voice -> dac pattern
   - Clear error messages for invalid configurations

4. **Extensible Architecture**
   - Easy to add new parallel patterns
   - Support for effects chains (future)
   - Modulation routing (future)

## Testing

✅ Builds successfully  
✅ All tests pass  
✅ Your dual-voice patch works  
✅ No warnings (except pre-existing Filter.sample_rate)

## Backward Compatibility

- Single-voice patches still work exactly as before
- `melody_params()` method provides backward-compatible access
- Existing examples unchanged

## What's Next

The architecture now supports:
- ✅ Multiple parallel voices
- ✅ Shared clock synchronization
- ✅ Automatic mixing
- 🔜 Effects chains
- 🔜 Modulation routing
- 🔜 Explicit mixer nodes with gain/pan controls
- 🔜 Send/return buses
