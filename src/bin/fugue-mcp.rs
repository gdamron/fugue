use std::sync::Arc;

use fugue::{
    default_sample_rate, ControlMeta, ControlValue, Invention, InventionBuilder, ModuleRegistry,
    ModuleSpec, RunningInvention,
};
use indexmap::IndexMap;
use rmcp::{
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::*,
    tool, tool_handler, tool_router, ServerHandler, ServiceExt,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Shadow state types (RunningInvention doesn't expose list APIs)
// ---------------------------------------------------------------------------

#[derive(Clone, Serialize)]
struct ModuleInfo {
    id: String,
    module_type: String,
    config: serde_json::Value,
}

#[derive(Clone, Serialize)]
struct ConnectionInfo {
    from: String,
    from_port: String,
    to: String,
    to_port: String,
}

struct FugueState {
    running: Option<RunningInvention>,
    modules: IndexMap<String, ModuleInfo>,
    connections: Vec<ConnectionInfo>,
    sample_rate: u32,
}

// ---------------------------------------------------------------------------
// MCP Server
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct FugueMcp {
    state: Arc<tokio::sync::Mutex<FugueState>>,
    tool_router: ToolRouter<Self>,
}

fn mcp_error(msg: impl Into<String>) -> ErrorData {
    let s: String = msg.into();
    ErrorData::new(ErrorCode::INTERNAL_ERROR, s, None)
}

fn graph_err(e: fugue::GraphCommandError) -> ErrorData {
    mcp_error(e.to_string())
}

fn json_result(value: &impl Serialize) -> Result<CallToolResult, ErrorData> {
    let text = serde_json::to_string_pretty(value).map_err(|e| mcp_error(e.to_string()))?;
    Ok(CallToolResult::success(vec![Content::text(text)]))
}

// ---------------------------------------------------------------------------
// Tool parameter types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
struct CreateInventionParams {
    #[schemars(description = "Optional title for the invention")]
    title: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct LoadInventionParams {
    #[schemars(description = "Complete invention JSON string (with modules, connections, etc.)")]
    json: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct AddModuleParams {
    #[schemars(
        description = "Unique module instance ID (e.g. 'clock1', 'osc_lead'). Use describe_module_types to see available types."
    )]
    id: String,
    #[schemars(
        description = "Module type: clock, oscillator, lfo, filter, mixer, adsr, vca, melody, step_sequencer, dac"
    )]
    module_type: String,
    #[schemars(description = "Optional JSON config object specific to the module type")]
    config: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct RemoveModuleParams {
    #[schemars(description = "Module instance ID to remove")]
    id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct ConnectParams {
    #[schemars(description = "Source module ID")]
    from: String,
    #[schemars(
        description = "Output port name on source (e.g. 'audio', 'gate', 'frequency', 'envelope')"
    )]
    from_port: String,
    #[schemars(description = "Destination module ID")]
    to: String,
    #[schemars(
        description = "Input port name on destination (e.g. 'audio', 'gate', 'frequency', 'cv', 'fm', 'am')"
    )]
    to_port: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct DisconnectParams {
    #[schemars(description = "Source module ID")]
    from: String,
    #[schemars(description = "Output port on source")]
    from_port: String,
    #[schemars(description = "Destination module ID")]
    to: String,
    #[schemars(description = "Input port on destination")]
    to_port: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct SetControlParams {
    #[schemars(description = "Module instance ID")]
    module_id: String,
    #[schemars(
        description = "Control key (e.g. 'bpm', 'attack', 'type'). Use list_controls to see available keys."
    )]
    key: String,
    #[schemars(description = "New value for the control")]
    value: serde_json::Value,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct GetControlParams {
    #[schemars(description = "Module instance ID")]
    module_id: String,
    #[schemars(description = "Control key")]
    key: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct ListControlsParams {
    #[schemars(description = "Module instance ID. If omitted, lists controls for all modules.")]
    module_id: Option<String>,
}

// ---------------------------------------------------------------------------
// Response types
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct StatusResponse {
    running: bool,
    module_count: usize,
    connection_count: usize,
    modules: Vec<String>,
}

#[derive(Serialize)]
struct ModuleTypeInfo {
    type_name: String,
    inputs: Vec<String>,
    outputs: Vec<String>,
    controls: Vec<ControlMeta>,
    is_sink: bool,
}

#[derive(Serialize)]
struct ControlEntry {
    module_id: String,
    controls: Vec<ControlMeta>,
}

fn parse_control_value(value: serde_json::Value) -> Result<ControlValue, ErrorData> {
    match value {
        serde_json::Value::Bool(value) => Ok(ControlValue::Bool(value)),
        serde_json::Value::Number(value) => value
            .as_f64()
            .map(|value| ControlValue::Number(value as f32))
            .ok_or_else(|| mcp_error("Numeric control values must fit in f32")),
        serde_json::Value::String(value) => Ok(ControlValue::String(value)),
        _ => Err(mcp_error(
            "Control value must be a JSON number, boolean, or string",
        )),
    }
}

// ---------------------------------------------------------------------------
// Tool implementations
// ---------------------------------------------------------------------------

#[tool_router]
impl FugueMcp {
    fn new(sample_rate: u32) -> Self {
        Self {
            state: Arc::new(tokio::sync::Mutex::new(FugueState {
                running: None,
                modules: IndexMap::new(),
                connections: Vec::new(),
                sample_rate,
            })),
            tool_router: Self::tool_router(),
        }
    }

    // -- Lifecycle --

    #[tool(
        description = "Create a minimal invention with just a DAC (audio output) module and start playback. Stops any currently running invention first. Add modules and connections to build your synthesis setup."
    )]
    async fn create_invention(
        &self,
        Parameters(params): Parameters<CreateInventionParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let mut state = self.state.lock().await;

        // Stop existing invention
        if let Some(running) = state.running.take() {
            running.stop();
        }
        state.modules.clear();
        state.connections.clear();

        let invention = Invention {
            version: "1.0.0".to_string(),
            title: params.title.clone(),
            description: None,
            developments: vec![],
            modules: vec![ModuleSpec {
                id: "dac".to_string(),
                module_type: "dac".to_string(),
                config: serde_json::Value::Null,
            }],
            connections: vec![],
            inputs: vec![],
            outputs: vec![],
            controls: vec![],
            source_path: None,
        };

        let builder = InventionBuilder::new(state.sample_rate);
        let (runtime, _handles) = builder
            .build(invention)
            .map_err(|e| mcp_error(e.to_string()))?;
        let running = runtime.start().map_err(|e| mcp_error(e.to_string()))?;

        state.modules.insert(
            "dac".to_string(),
            ModuleInfo {
                id: "dac".to_string(),
                module_type: "dac".to_string(),
                config: serde_json::Value::Null,
            },
        );
        state.running = Some(running);

        let title = params.title.as_deref().unwrap_or("untitled");
        Ok(CallToolResult::success(vec![Content::text(format!(
            "Created invention '{}' with DAC. Add modules and connect them to make sound.",
            title
        ))]))
    }

    #[tool(
        description = "Load an invention from a JSON string and start playback. The JSON should have 'modules' and 'connections' arrays. Stops any currently running invention first."
    )]
    async fn load_invention(
        &self,
        Parameters(params): Parameters<LoadInventionParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let mut state = self.state.lock().await;

        if let Some(running) = state.running.take() {
            running.stop();
        }
        state.modules.clear();
        state.connections.clear();

        let invention = Invention::from_json(&params.json).map_err(|e| mcp_error(e.to_string()))?;

        // Populate shadow state from parsed invention
        for spec in &invention.modules {
            state.modules.insert(
                spec.id.clone(),
                ModuleInfo {
                    id: spec.id.clone(),
                    module_type: spec.module_type.clone(),
                    config: spec.config.clone(),
                },
            );
        }
        for conn in &invention.connections {
            state.connections.push(ConnectionInfo {
                from: conn.from.clone(),
                from_port: conn.from_port.clone().unwrap_or_default(),
                to: conn.to.clone(),
                to_port: conn.to_port.clone().unwrap_or_default(),
            });
        }

        let title = invention.title.clone().unwrap_or("untitled".to_string());
        let module_count = invention.modules.len();
        let conn_count = invention.connections.len();

        let builder = InventionBuilder::new(state.sample_rate);
        let (runtime, _handles) = builder
            .build(invention)
            .map_err(|e| mcp_error(e.to_string()))?;
        let running = runtime.start().map_err(|e| mcp_error(e.to_string()))?;
        state.running = Some(running);

        Ok(CallToolResult::success(vec![Content::text(format!(
            "Loaded invention '{}' with {} modules and {} connections. Playback started.",
            title, module_count, conn_count
        ))]))
    }

    #[tool(description = "Stop the currently running invention and silence audio output.")]
    async fn stop_invention(&self) -> Result<CallToolResult, ErrorData> {
        let mut state = self.state.lock().await;
        if let Some(running) = state.running.take() {
            running.stop();
            state.modules.clear();
            state.connections.clear();
            Ok(CallToolResult::success(vec![Content::text(
                "Invention stopped.",
            )]))
        } else {
            Ok(CallToolResult::success(vec![Content::text(
                "No invention is running.",
            )]))
        }
    }

    #[tool(
        description = "Get the current status: whether an invention is running, module count, connection count, and module IDs."
    )]
    async fn get_status(&self) -> Result<CallToolResult, ErrorData> {
        let state = self.state.lock().await;
        let resp = StatusResponse {
            running: state.running.is_some(),
            module_count: state.modules.len(),
            connection_count: state.connections.len(),
            modules: state.modules.keys().cloned().collect(),
        };
        json_result(&resp)
    }

    // -- Modules --

    #[tool(
        description = "Add a module to the running invention. Common types: clock (tempo/gate output), oscillator (waveform generator), melody (algorithmic note sequencer), adsr (envelope generator), vca (voltage-controlled amplifier), lfo (low-frequency oscillator), filter (low/high/band-pass), mixer (multi-input mixer), step_sequencer (step-based sequencer), dac (audio output sink). Use describe_module_types for full port and control details."
    )]
    async fn add_module(
        &self,
        Parameters(params): Parameters<AddModuleParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let mut state = self.state.lock().await;
        let running = state
            .running
            .as_ref()
            .ok_or_else(|| mcp_error("No invention is running. Call create_invention first."))?;

        let config = params.config.clone().unwrap_or(serde_json::Value::Null);
        running
            .add_module(&params.id, &params.module_type, &config)
            .map_err(graph_err)?;

        state.modules.insert(
            params.id.clone(),
            ModuleInfo {
                id: params.id.clone(),
                module_type: params.module_type.clone(),
                config,
            },
        );

        Ok(CallToolResult::success(vec![Content::text(format!(
            "Added {} module '{}'.",
            params.module_type, params.id
        ))]))
    }

    #[tool(
        description = "Remove a module from the running invention. All its connections are also removed."
    )]
    async fn remove_module(
        &self,
        Parameters(params): Parameters<RemoveModuleParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let mut state = self.state.lock().await;
        let running = state
            .running
            .as_ref()
            .ok_or_else(|| mcp_error("No invention is running."))?;

        running.remove_module(&params.id).map_err(graph_err)?;

        state.modules.shift_remove(&params.id);
        state
            .connections
            .retain(|c| c.from != params.id && c.to != params.id);

        Ok(CallToolResult::success(vec![Content::text(format!(
            "Removed module '{}'.",
            params.id
        ))]))
    }

    #[tool(description = "List all modules currently in the invention with their types.")]
    async fn list_modules(&self) -> Result<CallToolResult, ErrorData> {
        let state = self.state.lock().await;
        let modules: Vec<&ModuleInfo> = state.modules.values().collect();
        json_result(&modules)
    }

    // -- Connections --

    #[tool(
        description = "Connect two modules by their ports. Signal flows from source output port to destination input port. Common patterns: clock:gate->melody:gate, melody:frequency->oscillator:frequency, oscillator:audio->vca:audio, adsr:envelope->vca:cv, vca:audio->dac:audio."
    )]
    async fn connect(
        &self,
        Parameters(params): Parameters<ConnectParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let mut state = self.state.lock().await;
        let running = state
            .running
            .as_ref()
            .ok_or_else(|| mcp_error("No invention is running."))?;

        running
            .connect(&params.from, &params.from_port, &params.to, &params.to_port)
            .map_err(graph_err)?;

        state.connections.push(ConnectionInfo {
            from: params.from.clone(),
            from_port: params.from_port.clone(),
            to: params.to.clone(),
            to_port: params.to_port.clone(),
        });

        Ok(CallToolResult::success(vec![Content::text(format!(
            "Connected {}:{} -> {}:{}",
            params.from, params.from_port, params.to, params.to_port
        ))]))
    }

    #[tool(
        description = "Disconnect two modules by removing the connection between specific ports."
    )]
    async fn disconnect(
        &self,
        Parameters(params): Parameters<DisconnectParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let mut state = self.state.lock().await;
        let running = state
            .running
            .as_ref()
            .ok_or_else(|| mcp_error("No invention is running."))?;

        running
            .disconnect(&params.from, &params.from_port, &params.to, &params.to_port)
            .map_err(graph_err)?;

        state.connections.retain(|c| {
            !(c.from == params.from
                && c.from_port == params.from_port
                && c.to == params.to
                && c.to_port == params.to_port)
        });

        Ok(CallToolResult::success(vec![Content::text(format!(
            "Disconnected {}:{} -> {}:{}",
            params.from, params.from_port, params.to, params.to_port
        ))]))
    }

    #[tool(description = "List all connections in the current invention.")]
    async fn list_connections(&self) -> Result<CallToolResult, ErrorData> {
        let state = self.state.lock().await;
        json_result(&state.connections)
    }

    // -- Controls --

    #[tool(
        description = "Set a control value on a module (e.g. BPM on a clock, attack time on an ADSR, waveform type on an oscillator). Use list_controls to discover available controls and their valid ranges."
    )]
    async fn set_control(
        &self,
        Parameters(params): Parameters<SetControlParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let state = self.state.lock().await;
        let running = state
            .running
            .as_ref()
            .ok_or_else(|| mcp_error("No invention is running."))?;

        running
            .set_control(
                &params.module_id,
                &params.key,
                parse_control_value(params.value.clone())?,
            )
            .map_err(graph_err)?;

        Ok(CallToolResult::success(vec![Content::text(format!(
            "Set {}.{}",
            params.module_id, params.key
        ))]))
    }

    #[tool(description = "Get the current value of a control on a module.")]
    async fn get_control(
        &self,
        Parameters(params): Parameters<GetControlParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let state = self.state.lock().await;
        let running = state
            .running
            .as_ref()
            .ok_or_else(|| mcp_error("No invention is running."))?;

        let value = running
            .get_control(&params.module_id, &params.key)
            .map_err(graph_err)?;

        Ok(CallToolResult::success(vec![Content::text(format!(
            "{}.{} = {}",
            params.module_id,
            params.key,
            serde_json::to_string(&value).map_err(|e| mcp_error(e.to_string()))?
        ))]))
    }

    #[tool(
        description = "List available controls for a module (or all modules if module_id is omitted). Shows control key, description, min/max range, default value, and enum variants if applicable."
    )]
    async fn list_controls(
        &self,
        Parameters(params): Parameters<ListControlsParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let state = self.state.lock().await;
        let running = state
            .running
            .as_ref()
            .ok_or_else(|| mcp_error("No invention is running."))?;

        if let Some(module_id) = &params.module_id {
            let controls = running.list_controls(module_id).map_err(graph_err)?;
            let entry = ControlEntry {
                module_id: module_id.clone(),
                controls,
            };
            json_result(&vec![entry])
        } else {
            let all = running.list_all_controls();
            let entries: Vec<ControlEntry> = all
                .into_iter()
                .map(|(id, controls)| ControlEntry {
                    module_id: id,
                    controls,
                })
                .collect();
            json_result(&entries)
        }
    }

    // -- Discovery --

    #[tool(
        description = "Describe all available module types with their input ports, output ports, and controls. This is the key discovery tool — call it first to understand what modules exist and how to connect them."
    )]
    async fn describe_module_types(&self) -> Result<CallToolResult, ErrorData> {
        let state = self.state.lock().await;
        let registry = ModuleRegistry::default();
        let sample_rate = state.sample_rate;

        let mut types: Vec<ModuleTypeInfo> = Vec::new();
        let mut type_names: Vec<&str> = registry.types().collect();
        type_names.sort();

        for type_name in type_names {
            let config = serde_json::Value::Null;
            match registry.build(type_name, sample_rate, &config) {
                Ok(result) => {
                    let module = result.module.lock().unwrap();
                    let inputs: Vec<String> =
                        module.inputs().iter().map(|s| s.to_string()).collect();
                    let outputs: Vec<String> =
                        module.outputs().iter().map(|s| s.to_string()).collect();
                    let controls = result
                        .control_surface
                        .as_ref()
                        .map(|surface| surface.controls())
                        .unwrap_or_default();

                    types.push(ModuleTypeInfo {
                        type_name: type_name.to_string(),
                        inputs,
                        outputs,
                        controls,
                        is_sink: registry.is_sink(type_name),
                    });
                }
                Err(_) => {
                    // Module requires config to build — just list it with empty ports
                    types.push(ModuleTypeInfo {
                        type_name: type_name.to_string(),
                        inputs: vec![],
                        outputs: vec![],
                        controls: vec![],
                        is_sink: registry.is_sink(type_name),
                    });
                }
            }
        }

        json_result(&types)
    }
}

// ---------------------------------------------------------------------------
// ServerHandler
// ---------------------------------------------------------------------------

#[tool_handler]
impl ServerHandler for FugueMcp {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("fugue-mcp", env!("CARGO_PKG_VERSION")))
            .with_instructions(
                "Fugue modular synthesis server. Create inventions, add modules (oscillators, \
                 filters, envelopes, etc.), connect them via named ports, and adjust controls \
                 in real time. Call describe_module_types first to see available modules and \
                 their ports. Typical signal chain: clock -> melody -> oscillator -> vca -> dac, \
                 with an adsr envelope controlling the vca's cv input.",
            )
    }
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .init();

    let sample_rate = default_sample_rate().unwrap_or(44100);
    let server = FugueMcp::new(sample_rate)
        .serve(rmcp::transport::stdio())
        .await?;
    server.waiting().await?;
    Ok(())
}
