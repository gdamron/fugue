# Wasm modules

A `wasm_module` runs a guest component as a graph module. This document is
for anyone building a guest, and for contributors changing the host side
([`src/plugins/wasm.rs`](../src/plugins/wasm.rs)).

## The world

A guest implements the `module` world in
[`wit/fugue-module.wit`](../wit/fugue-module.wit), now `fugue:module@0.2.0`.
The host calls the guest's exports, and the guest calls one host import:

- `init`, `name`, `inputs` and `outputs` run once, when the module loads.
- `set-input-block` hands the guest each connected input's block, then
  `process` renders the block.
- Inside `process`, the guest calls `emit-output(index, samples)` once for
  each output. The host copies the samples into a buffer it allocated when
  the module loaded.
- `set-input` and `set-control` take single values from controls and tests.

`process` runs on the audio thread for every block. The host side of each
call allocates nothing, frees nothing and takes no lock. A test checks this
for every module type, `wasm_module` included. Keep the guest's own block
path free of allocation, too: render into a buffer the guest owns and emit a
slice of it.

### Writing outputs

- An output not written during a block reads as silence for that block.
- A list shorter than the block is padded with silence. Samples past the
  block are ignored.
- A second call for the same output in one block replaces the first.
- A call from outside `process`, or with an index past `outputs`, fails the
  module. It is muted from then on, and the reason is printed.

## Building a guest

The fixture guest in
[`tests/fixtures/fugue-module-guest`](../tests/fixtures/fugue-module-guest)
is the smallest complete guest, and a good template. With `wit-bindgen`:

```rust
wit_bindgen::generate!({ path: "wit", world: "module" });

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

The host loads guests built for exactly one version of the world. It reads
the version from the guest's import of the world's `host` interface,
`fugue:module/host@0.2.0`, before it runs any guest code. A guest built for
another version is refused with a message that names the version it was
built for and the one it needs.

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
2. Delete the `process-output-block` and `output-block` exports.
3. At the end of `process`, call `emit-output` once for each output, with
   the samples `output-block` used to return.
4. Rebuild the component.

The exports `init`, `name`, `inputs`, `outputs`, `set-input`,
`set-input-connected`, `set-input-block`, `process`, `get-output` and
`set-control` are unchanged. A guest that did its rendering in `process`
and returned a stored block from `output-block` sounds the same after the
move.
