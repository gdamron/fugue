# Wasm modules

A `wasm_module` runs a guest component as a graph module. This document is
for anyone building a guest, and for contributors changing the host side
([`src/plugins/wasm.rs`](../src/plugins/wasm.rs)).

## The world

A guest implements the `module` world in
[`wit/fugue-module.wit`](../wit/fugue-module.wit), now `fugue:module@0.2.0`.
The guest exports the `guest` interface, whose functions the host calls,
and imports the `host` interface, whose one function the guest calls:

- `init`, `name`, `inputs` and `outputs` run once, when the module loads.
- `set-input-block` hands the guest each connected input's block, then
  `process` renders the block.
- Inside `process`, the guest calls `emit-output(index, samples)` once for
  each output. The host copies the samples into a buffer it allocated when
  the module loaded.
- `set-input` and `set-control` take single values from controls and tests.

The host reads outputs only through `emit-output`; there is no export for
reading a single output value.

`process` runs on the audio thread for every block. The host side of each
of these calls, `emit-output` included, allocates nothing, frees nothing and
takes no lock. A test checks this for every module type, `wasm_module`
included. Keep the guest's own block path free of allocation, too: render
into a buffer the guest owns and emit a slice of it. Don't call WASI
imports (random, clocks, stdio) from `process`: their host side allocates or
locks.

### Writing outputs

- An output not written during a block reads as silence for that block.
- A list shorter than the block is padded with silence. Samples past the
  block are ignored.
- A second call for the same output in one block replaces the first.
- A call with an index past `outputs` fails the module: it is muted from
  then on, and the reason is printed. So does a call from outside `process`:
  from `init` it fails the load, and from `set-input` or `set-control` it
  fails that call, and the module mutes at its next block.

## Building a guest

The fixture guest in
[`tests/fixtures/fugue-module-guest`](../tests/fixtures/fugue-module-guest)
is the smallest complete guest, and a good template. With `wit-bindgen`:

```rust
wit_bindgen::generate!({ path: "wit", world: "module" });

use exports::fugue::module::guest::Guest;

impl Guest for MyModule {
    fn process(frames: u32) -> bool {
        // Render `frames` samples into a buffer the guest owns...
        fugue::module::host::emit_output(0, &buffer[..frames as usize]);
        true
    }
    // ...the other exports.
}

export!(MyModule);
```

Build the crate for `wasm32-unknown-unknown`, then turn the core module into
a component, for example with `wasm-tools component new`. Name the file
`<name>.fugue-module.wasm` and point a `module` package manifest's
`entry.wasm` at it.

## Versions

The host loads guests built for exactly one version of the world, so every
new version is a breaking one. Before it runs any guest code, it reads the
version from the guest's export of the world's `guest` interface,
`fugue:module/guest@0.2.0`. Every guest has that export, whatever its inputs
and outputs, so every guest is checked. (An import would not do: a guest's
build drops imports it never calls, so a guest with no outputs would have no
`host` import to read.) A 0.1.0 guest exported its functions directly, with
no versioned interface, and is known by its `output-block` export. A guest
built for another version is refused with a message that names the version
it was built for and the one it needs.

### Migrating from 0.1.0

The first world, `fugue:module@0.1.0`, had guests return their output blocks.
The host had to allocate a fresh list to read each one, every block, on the
audio thread. A 0.1.0 guest is now refused:

```text
this wasm module was built for the fugue:module@0.1.0 world, and this host
loads fugue:module@0.2.0 modules: rebuild it against fugue:module@0.2.0
(wit/fugue-module.wit), which writes each output block with the host's
`emit-output` import
```

To move a guest to 0.2.0:

1. Take the new `wit/fugue-module.wit`.
2. Move the guest's functions into the `guest` interface. With
   `wit-bindgen`, implement `exports::fugue::module::guest::Guest` instead of
   the world's `Guest`.
3. Delete the `process-output-block`, `output-block` and `get-output`
   exports.
4. At the end of `process`, call `emit-output` once for each output, with
   the samples `output-block` used to return.
5. Rebuild the component.

The functions `init`, `name`, `inputs`, `outputs`, `set-input`,
`set-input-connected`, `set-input-block`, `process` and `set-control` keep
their signatures; only where they are exported from changes. A guest that
did its rendering in `process` and returned a stored block from
`output-block` sounds the same after the move.
