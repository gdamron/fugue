//! What the host gives a guest: its WASI context, scoped by the package's
//! capabilities, and the `emit-output` import, which copies each output
//! block into a buffer the host allocated when the module loaded.

use std::error::Error;
use std::path::Path;

use wasmtime::component::{Linker, ResourceTable, WasmList};
use wasmtime::{AsContextMut, StoreContextMut};
use wasmtime_wasi::{WasiCtx, WasiCtxView, WasiView};

use super::world::HOST_INTERFACE;
use crate::pkg::{Capability, PackageManifest};
use crate::MAX_BLOCK;

pub(super) struct HostState {
    table: ResourceTable,
    wasi: WasiCtx,
    pub(super) outputs: OutputBlocks,
}

impl HostState {
    pub(super) fn from_manifest(manifest: &PackageManifest) -> Result<Self, Box<dyn Error>> {
        let mut builder = WasiCtx::builder();
        for cap in &manifest.requires.capabilities {
            match Capability::parse(cap).ok_or_else(|| format!("invalid capability: {cap}"))? {
                Capability::Random | Capability::Time => {}
                Capability::FsRead(scope) => {
                    preopen(&mut builder, &scope, false)?;
                }
                Capability::FsWrite(scope) => {
                    preopen(&mut builder, &scope, true)?;
                }
                Capability::Net(host) => {
                    return Err(format!(
                        "network capability is not supported for wasm modules yet: {host}"
                    )
                    .into());
                }
            }
        }

        Ok(Self {
            table: ResourceTable::new(),
            wasi: builder.build(),
            outputs: OutputBlocks::default(),
        })
    }
}

impl WasiView for HostState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

fn preopen(
    builder: &mut wasmtime_wasi::WasiCtxBuilder,
    scope: &str,
    writable: bool,
) -> Result<(), Box<dyn Error>> {
    let path = Path::new(scope);
    let perms = if writable {
        wasmtime_wasi::FsPerms::ReadWrite
    } else {
        wasmtime_wasi::FsPerms::ReadOnly
    };
    builder.preopened_dir(path, scope, perms)?;
    Ok(())
}

/// The module's output blocks, which the guest fills through `emit-output`
/// while `process` runs.
#[derive(Default)]
pub(super) struct OutputBlocks {
    pub(super) blocks: Vec<[f32; MAX_BLOCK]>,
    /// The frames of the block `process` is rendering; `None` outside it.
    frames: Option<usize>,
}

impl OutputBlocks {
    pub(super) fn new(outputs: usize) -> Self {
        Self {
            blocks: vec![[0.0; MAX_BLOCK]; outputs],
            frames: None,
        }
    }

    /// Opens a block of `frames`: each output reads as silence until the
    /// guest writes it.
    pub(super) fn begin(&mut self, frames: usize) {
        for block in &mut self.blocks {
            block[..frames].fill(0.0);
        }
        self.frames = Some(frames);
    }

    pub(super) fn end(&mut self) {
        self.frames = None;
    }

    /// Copies `samples` into output `index`, padding a short list with
    /// silence. Reads the list in place in the guest's memory.
    fn write(
        &mut self,
        store: StoreContextMut<'_, HostState>,
        index: u32,
        samples: &WasmList<f32>,
    ) -> wasmtime::Result<()> {
        let Some(frames) = self.frames else {
            wasmtime::bail!("emit-output was called outside `process`");
        };
        let outputs = self.blocks.len();
        let Some(block) = self.blocks.get_mut(index as usize) else {
            wasmtime::bail!("emit-output index {index} is past the module's {outputs} outputs");
        };
        let written = frames.min(samples.len());
        for (slot, sample) in block[..written].iter_mut().zip(samples.iter(store)?) {
            *slot = sample?;
        }
        block[written..frames].fill(0.0);
        Ok(())
    }
}

/// Defines the `host` interface the guest world imports.
pub(super) fn add_to_linker(linker: &mut Linker<HostState>) -> wasmtime::Result<()> {
    linker.instance(HOST_INTERFACE)?.func_wrap(
        "emit-output",
        |mut store: StoreContextMut<'_, HostState>, (index, samples): (u32, WasmList<f32>)| {
            // Taken out for the copy, so the guest's memory can be read
            // through the store; moving a `Vec` neither allocates nor frees.
            let mut outputs = std::mem::take(&mut store.data_mut().outputs);
            let written = outputs.write(store.as_context_mut(), index, &samples);
            store.data_mut().outputs = outputs;
            written
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pkg::{Author, EntrySpec, PackageKind, Requires, Target};

    fn manifest_with_caps(capabilities: Vec<String>) -> PackageManifest {
        PackageManifest {
            id: "fugue.test.module".to_string(),
            version: "1.0.0".to_string(),
            kind: PackageKind::Module,
            license: "MIT".to_string(),
            authors: vec![Author {
                name: "Fugue Test".to_string(),
                url: None,
            }],
            description: None,
            homepage: None,
            targets: vec![Target::InGraphAgent],
            requires: Requires {
                mcp_tools: Vec::new(),
                capabilities,
            },
            deps: Vec::new(),
            entry: EntrySpec::Module {
                wasm: "module.fugue-module.wasm".to_string(),
            },
            signing: None,
        }
    }

    #[test]
    fn host_state_rejects_unsupported_network_capability() {
        let manifest = manifest_with_caps(vec!["net:example.com".to_string()]);
        let err = match HostState::from_manifest(&manifest) {
            Ok(_) => panic!("network capability should be rejected"),
            Err(err) => err.to_string(),
        };
        assert!(err.contains("network capability is not supported"));
    }

    #[test]
    fn host_state_accepts_random_and_time_without_extra_grants() {
        let manifest = manifest_with_caps(vec!["random".to_string(), "time".to_string()]);
        HostState::from_manifest(&manifest).expect("random/time capabilities are valid");
    }

    /// A guest whose `emit(index, len)` export calls `emit-output` with the
    /// first `len` of the samples 1.0, 2.0, 3.0.
    const EMITTER: &str = r#"(component
        (import "fugue:module/host@0.2.0" (instance $host
            (export "emit-output" (func (param "index" u32) (param "samples" (list f32))))))
        (core module $libc
            (memory (export "memory") 1)
            (data (i32.const 0) "\00\00\80\3f\00\00\00\40\00\00\40\40"))
        (core instance $libc (instantiate $libc))
        (alias core export $libc "memory" (core memory $memory))
        (core func $emit_output (canon lower (func $host "emit-output") (memory $memory)))
        (core module $guest
            (import "host" "emit-output" (func $emit_output (param i32 i32 i32)))
            (func (export "emit") (param $index i32) (param $len i32)
                local.get $index
                i32.const 0
                local.get $len
                call $emit_output))
        (core instance $guest (instantiate $guest
            (with "host" (instance (export "emit-output" (func $emit_output))))))
        (func (export "emit") (param "index" u32) (param "len" u32)
            (canon lift (core func $guest "emit"))))"#;

    type Emit = wasmtime::component::TypedFunc<(u32, u32), ()>;

    /// An emitter instance with two outputs, each block filled with 9.0.
    fn emitter() -> (wasmtime::Store<HostState>, Emit) {
        let engine = super::super::component_engine().expect("engine");
        let component = wasmtime::component::Component::new(&engine, EMITTER).expect("component");
        let mut linker = Linker::new(&engine);
        add_to_linker(&mut linker).expect("host interface");
        let manifest = manifest_with_caps(Vec::new());
        let mut store = wasmtime::Store::new(&engine, HostState::from_manifest(&manifest).unwrap());
        store.set_fuel(u64::MAX / 2).unwrap();
        let instance = linker
            .instantiate(&mut store, &component)
            .expect("instance");
        let emit = instance.get_typed_func(&mut store, "emit").expect("emit");
        store.data_mut().outputs = OutputBlocks::new(2);
        for block in &mut store.data_mut().outputs.blocks {
            block.fill(9.0);
        }
        (store, emit)
    }

    fn emit(store: &mut wasmtime::Store<HostState>, emit: &Emit, index: u32, len: u32) -> String {
        match emit.call(&mut *store, (index, len)) {
            Ok(()) => String::new(),
            // As the module reports it when it fails.
            Err(error) => {
                let signal = super::super::failure::FailureSignal::default();
                signal.name("emitter");
                signal.record("process", super::super::failure::Reason::Trap(error));
                signal.message().expect("a recorded failure")
            }
        }
    }

    #[test]
    fn emitted_samples_fill_the_block_padded_with_silence() {
        let (mut store, func) = emitter();
        store.data_mut().outputs.begin(4);
        assert_eq!(emit(&mut store, &func, 0, 2), "");
        let blocks = &store.data().outputs.blocks;
        // Past the block's frames, nothing is touched.
        assert_eq!(&blocks[0][..5], &[1.0, 2.0, 0.0, 0.0, 9.0]);
        // An output not written in the block reads as silence.
        assert_eq!(&blocks[1][..5], &[0.0, 0.0, 0.0, 0.0, 9.0]);

        store.data_mut().outputs.begin(2);
        assert_eq!(emit(&mut store, &func, 1, 3), "");
        // Samples past the block are ignored.
        assert_eq!(&store.data().outputs.blocks[1][..3], &[1.0, 2.0, 0.0]);
    }

    #[test]
    fn an_index_past_the_outputs_fails_naming_the_reason() {
        let (mut store, func) = emitter();
        store.data_mut().outputs.begin(4);
        let message = emit(&mut store, &func, 2, 1);
        assert!(
            message.contains("index 2 is past the module's 2 outputs"),
            "{message}"
        );
    }

    #[test]
    fn emitting_outside_process_fails() {
        let (mut store, func) = emitter();
        store.data_mut().outputs.begin(4);
        store.data_mut().outputs.end();
        store.data_mut().outputs.blocks[0][0] = 9.0;
        let error = emit(&mut store, &func, 0, 1);
        assert!(error.contains("called outside `process`"), "{error}");
        assert_eq!(store.data().outputs.blocks[0][0], 9.0);
    }
}
