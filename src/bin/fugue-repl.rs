use fugue::{
    default_sample_rate, Connection, ControlKind, ControlMeta, ControlValue, Invention,
    InventionBuilder, ModuleRegistry, ModuleSpec, RunningInvention,
};
use indexmap::IndexMap;
use rustyline::error::ReadlineError;
use rustyline::DefaultEditor;

// ---------------------------------------------------------------------------
// Shadow state (mirrors fugue-mcp pattern)
// ---------------------------------------------------------------------------

struct ModuleInfo {
    id: String,
    module_type: String,
    config: serde_json::Value,
}

struct ConnectionInfo {
    from: String,
    from_port: String,
    to: String,
    to_port: String,
}

struct FugueRepl {
    running: Option<RunningInvention>,
    modules: IndexMap<String, ModuleInfo>,
    connections: Vec<ConnectionInfo>,
    title: Option<String>,
    sample_rate: u32,
}

impl FugueRepl {
    fn new(sample_rate: u32) -> Self {
        Self {
            running: None,
            modules: IndexMap::new(),
            connections: Vec::new(),
            title: None,
            sample_rate,
        }
    }

    fn stop_current(&mut self) {
        if let Some(running) = self.running.take() {
            running.stop();
        }
        self.modules.clear();
        self.connections.clear();
        self.title = None;
    }

    fn to_invention(&self) -> Invention {
        Invention {
            version: "1.0.0".to_string(),
            title: self.title.clone(),
            description: None,
            modules: self
                .modules
                .values()
                .map(|m| ModuleSpec {
                    id: m.id.clone(),
                    module_type: m.module_type.clone(),
                    config: m.config.clone(),
                })
                .collect(),
            connections: self
                .connections
                .iter()
                .map(|c| Connection {
                    from: c.from.clone(),
                    to: c.to.clone(),
                    from_port: Some(c.from_port.clone()),
                    to_port: Some(c.to_port.clone()),
                })
                .collect(),
        }
    }

    fn require_running(&self) -> Result<&RunningInvention, String> {
        self.running
            .as_ref()
            .ok_or_else(|| "No invention is running. Use 'new' or 'load' first.".to_string())
    }
}

// ---------------------------------------------------------------------------
// Command dispatch
// ---------------------------------------------------------------------------

fn execute(repl: &mut FugueRepl, line: &str) -> Result<String, String> {
    let line = line.trim();
    if line.is_empty() {
        return Ok(String::new());
    }

    let parts: Vec<&str> = line.splitn(2, char::is_whitespace).collect();
    let cmd = parts[0];
    let rest = parts.get(1).map(|s| s.trim()).unwrap_or("");

    match cmd {
        "new" => cmd_new(repl, rest),
        "load" => cmd_load(repl, rest),
        "load-json" => cmd_load_json(repl, rest),
        "stop" => cmd_stop(repl),
        "status" => cmd_status(repl),
        "add" => cmd_add(repl, rest),
        "remove" => cmd_remove(repl, rest),
        "modules" => cmd_modules(repl),
        "connect" => cmd_connect(repl, rest),
        "disconnect" => cmd_disconnect(repl, rest),
        "connections" => cmd_connections(repl),
        "set" => cmd_set(repl, rest),
        "get" => cmd_get(repl, rest),
        "controls" => cmd_controls(repl, rest),
        "save" => cmd_save(repl, rest),
        "types" => cmd_types(repl),
        "help" => Ok(help_text()),
        "quit" | "exit" => {
            dev_save_state(repl);
            repl.stop_current();
            std::process::exit(0);
        }
        _ => Err(format!(
            "Unknown command: '{}'. Type 'help' for usage.",
            cmd
        )),
    }
}

// ---------------------------------------------------------------------------
// Lifecycle commands
// ---------------------------------------------------------------------------

fn cmd_new(repl: &mut FugueRepl, rest: &str) -> Result<String, String> {
    repl.stop_current();

    let title = if rest.is_empty() {
        None
    } else {
        Some(rest.to_string())
    };

    let invention = Invention {
        version: "1.0.0".to_string(),
        title: title.clone(),
        description: None,
        modules: vec![ModuleSpec {
            id: "dac".to_string(),
            module_type: "dac".to_string(),
            config: serde_json::Value::Null,
        }],
        connections: vec![],
    };

    let builder = InventionBuilder::new(repl.sample_rate);
    let (runtime, _handles) = builder.build(invention).map_err(|e| e.to_string())?;
    let running = runtime.start().map_err(|e| e.to_string())?;

    repl.modules.insert(
        "dac".to_string(),
        ModuleInfo {
            id: "dac".to_string(),
            module_type: "dac".to_string(),
            config: serde_json::Value::Null,
        },
    );
    repl.running = Some(running);
    repl.title = title.clone();

    let name = title.as_deref().unwrap_or("untitled");
    Ok(format!("Created invention '{}' with DAC.", name))
}

fn cmd_load(repl: &mut FugueRepl, rest: &str) -> Result<String, String> {
    if rest.is_empty() {
        return Err("Usage: load <path>".to_string());
    }
    let invention = Invention::from_file(rest).map_err(|e| e.to_string())?;
    start_invention(repl, invention)
}

fn cmd_load_json(repl: &mut FugueRepl, rest: &str) -> Result<String, String> {
    if rest.is_empty() {
        return Err("Usage: load-json <json>".to_string());
    }
    let invention = Invention::from_json(rest).map_err(|e| e.to_string())?;
    start_invention(repl, invention)
}

fn start_invention(repl: &mut FugueRepl, invention: Invention) -> Result<String, String> {
    repl.stop_current();

    // Populate shadow state
    repl.title = invention.title.clone();
    for spec in &invention.modules {
        repl.modules.insert(
            spec.id.clone(),
            ModuleInfo {
                id: spec.id.clone(),
                module_type: spec.module_type.clone(),
                config: spec.config.clone(),
            },
        );
    }
    for conn in &invention.connections {
        repl.connections.push(ConnectionInfo {
            from: conn.from.clone(),
            from_port: conn.from_port.clone().unwrap_or_default(),
            to: conn.to.clone(),
            to_port: conn.to_port.clone().unwrap_or_default(),
        });
    }

    let title = invention
        .title
        .clone()
        .unwrap_or_else(|| "untitled".to_string());
    let module_count = invention.modules.len();
    let conn_count = invention.connections.len();

    let builder = InventionBuilder::new(repl.sample_rate);
    let (runtime, _handles) = builder.build(invention).map_err(|e| e.to_string())?;
    let running = runtime.start().map_err(|e| e.to_string())?;
    repl.running = Some(running);

    Ok(format!(
        "Loaded '{}' with {} modules, {} connections.",
        title, module_count, conn_count
    ))
}

fn cmd_stop(repl: &mut FugueRepl) -> Result<String, String> {
    if repl.running.is_some() {
        repl.stop_current();
        Ok("Stopped.".to_string())
    } else {
        Ok("No invention is running.".to_string())
    }
}

fn cmd_status(repl: &FugueRepl) -> Result<String, String> {
    if repl.running.is_some() {
        Ok(format!(
            "Running: {} modules, {} connections",
            repl.modules.len(),
            repl.connections.len()
        ))
    } else {
        Ok("Not running.".to_string())
    }
}

fn cmd_save(repl: &FugueRepl, rest: &str) -> Result<String, String> {
    if rest.is_empty() {
        return Err("Usage: save <path>".to_string());
    }
    if repl.modules.is_empty() {
        return Err("Nothing to save. No modules in current state.".to_string());
    }

    let invention = repl.to_invention();
    let json = invention.to_json().map_err(|e| e.to_string())?;
    std::fs::write(rest, &json).map_err(|e| e.to_string())?;

    Ok(format!("Saved to {}.", rest))
}

// ---------------------------------------------------------------------------
// Module commands
// ---------------------------------------------------------------------------

fn cmd_add(repl: &mut FugueRepl, rest: &str) -> Result<String, String> {
    let running = repl.require_running()?;

    // Split: <id> <type> [config_json...]
    let parts: Vec<&str> = rest.splitn(3, char::is_whitespace).collect();
    if parts.len() < 2 {
        return Err("Usage: add <id> <type> [config_json]".to_string());
    }

    let id = parts[0];
    let module_type = parts[1];
    let config: serde_json::Value = if parts.len() > 2 {
        serde_json::from_str(parts[2]).map_err(|e| format!("Invalid config JSON: {}", e))?
    } else {
        serde_json::Value::Null
    };

    running
        .add_module(id, module_type, &config)
        .map_err(|e| e.to_string())?;

    repl.modules.insert(
        id.to_string(),
        ModuleInfo {
            id: id.to_string(),
            module_type: module_type.to_string(),
            config: config.clone(),
        },
    );

    Ok(format!("Added {} '{}'.", module_type, id))
}

fn cmd_remove(repl: &mut FugueRepl, rest: &str) -> Result<String, String> {
    let running = repl.require_running()?;

    let id = rest.split_whitespace().next().ok_or("Usage: remove <id>")?;

    running.remove_module(id).map_err(|e| e.to_string())?;

    repl.modules.shift_remove(id);
    repl.connections.retain(|c| c.from != id && c.to != id);

    Ok(format!("Removed '{}'.", id))
}

fn cmd_modules(repl: &FugueRepl) -> Result<String, String> {
    if repl.modules.is_empty() {
        return Ok("No modules.".to_string());
    }
    let mut out = String::new();
    for info in repl.modules.values() {
        out.push_str(&format!("  {:<16} {}\n", info.id, info.module_type));
    }
    Ok(out.trim_end().to_string())
}

// ---------------------------------------------------------------------------
// Connection commands
// ---------------------------------------------------------------------------

fn cmd_connect(repl: &mut FugueRepl, rest: &str) -> Result<String, String> {
    let running = repl.require_running()?;

    let parts: Vec<&str> = rest.split_whitespace().collect();
    if parts.len() != 4 {
        return Err("Usage: connect <from> <from_port> <to> <to_port>".to_string());
    }

    running
        .connect(parts[0], parts[1], parts[2], parts[3])
        .map_err(|e| e.to_string())?;

    repl.connections.push(ConnectionInfo {
        from: parts[0].to_string(),
        from_port: parts[1].to_string(),
        to: parts[2].to_string(),
        to_port: parts[3].to_string(),
    });

    Ok(format!(
        "Connected {}:{} -> {}:{}",
        parts[0], parts[1], parts[2], parts[3]
    ))
}

fn cmd_disconnect(repl: &mut FugueRepl, rest: &str) -> Result<String, String> {
    let running = repl.require_running()?;

    let parts: Vec<&str> = rest.split_whitespace().collect();
    if parts.len() != 4 {
        return Err("Usage: disconnect <from> <from_port> <to> <to_port>".to_string());
    }

    running
        .disconnect(parts[0], parts[1], parts[2], parts[3])
        .map_err(|e| e.to_string())?;

    repl.connections.retain(|c| {
        !(c.from == parts[0]
            && c.from_port == parts[1]
            && c.to == parts[2]
            && c.to_port == parts[3])
    });

    Ok(format!(
        "Disconnected {}:{} -> {}:{}",
        parts[0], parts[1], parts[2], parts[3]
    ))
}

fn cmd_connections(repl: &FugueRepl) -> Result<String, String> {
    if repl.connections.is_empty() {
        return Ok("No connections.".to_string());
    }
    let mut out = String::new();
    for c in &repl.connections {
        out.push_str(&format!(
            "  {}:{} -> {}:{}\n",
            c.from, c.from_port, c.to, c.to_port
        ));
    }
    Ok(out.trim_end().to_string())
}

// ---------------------------------------------------------------------------
// Control commands
// ---------------------------------------------------------------------------

fn cmd_set(repl: &FugueRepl, rest: &str) -> Result<String, String> {
    let running = repl.require_running()?;

    let parts: Vec<&str> = rest.splitn(3, char::is_whitespace).collect();
    if parts.len() != 3 {
        return Err("Usage: set <module_id> <key> <value>".to_string());
    }

    let value: ControlValue =
        serde_json::from_str(parts[2]).map_err(|e| format!("Invalid control value JSON: {}", e))?;

    running
        .set_control(parts[0], parts[1], value)
        .map_err(|e| e.to_string())?;

    Ok(format!("{}.{} updated", parts[0], parts[1]))
}

fn cmd_get(repl: &FugueRepl, rest: &str) -> Result<String, String> {
    let running = repl.require_running()?;

    let parts: Vec<&str> = rest.split_whitespace().collect();
    if parts.len() != 2 {
        return Err("Usage: get <module_id> <key>".to_string());
    }

    let value = running
        .get_control(parts[0], parts[1])
        .map_err(|e| e.to_string())?;

    Ok(format!(
        "{}.{} = {}",
        parts[0],
        parts[1],
        serde_json::to_string(&value).map_err(|e| e.to_string())?
    ))
}

fn cmd_controls(repl: &FugueRepl, rest: &str) -> Result<String, String> {
    let running = repl.require_running()?;

    let module_id = rest.split_whitespace().next();

    if let Some(id) = module_id {
        let controls = running.list_controls(id).map_err(|e| e.to_string())?;
        if controls.is_empty() {
            return Ok(format!("{}: no controls", id));
        }
        let mut out = format!("{}:\n", id);
        for c in &controls {
            out.push_str(&format_control(c));
        }
        Ok(out.trim_end().to_string())
    } else {
        let all = running.list_all_controls();
        if all.is_empty() {
            return Ok("No controls.".to_string());
        }
        let mut out = String::new();
        for (id, controls) in &all {
            out.push_str(&format!("{}:\n", id));
            for c in controls {
                out.push_str(&format_control(c));
            }
        }
        Ok(out.trim_end().to_string())
    }
}

fn format_control(c: &ControlMeta) -> String {
    let default = serde_json::to_string(&c.default).unwrap_or_else(|_| "null".to_string());
    let details = match &c.kind {
        ControlKind::Number { min, max } => format!("range {}..{}, default {}", min, max, default),
        ControlKind::Bool => format!("bool, default {}", default),
        ControlKind::String { options } => {
            let options = options
                .as_ref()
                .map(|values| format!(" [{}]", values.join(", ")))
                .unwrap_or_default();
            format!("string{}, default {}", options, default)
        }
    };
    format!("    {:<14} {:<28} {}\n", c.key, details, c.description,)
}

// ---------------------------------------------------------------------------
// Discovery
// ---------------------------------------------------------------------------

fn cmd_types(repl: &FugueRepl) -> Result<String, String> {
    let registry = ModuleRegistry::default();
    let mut type_names: Vec<&str> = registry.types().collect();
    type_names.sort();

    let mut out = String::new();
    for type_name in type_names {
        let config = serde_json::Value::Null;
        match registry.build(type_name, repl.sample_rate, &config) {
            Ok(result) => {
                let module = result.module.lock().unwrap();
                let inputs: Vec<&str> = module.inputs().to_vec();
                let outputs: Vec<&str> = module.outputs().to_vec();
                let controls = result
                    .control_surface
                    .as_ref()
                    .map(|surface| surface.controls())
                    .unwrap_or_default();

                out.push_str(&format!("{}:\n", type_name));
                if !inputs.is_empty() {
                    out.push_str(&format!("  inputs:   {}\n", inputs.join(", ")));
                }
                if !outputs.is_empty() {
                    out.push_str(&format!("  outputs:  {}\n", outputs.join(", ")));
                }
                if !controls.is_empty() {
                    out.push_str("  controls:\n");
                    for c in &controls {
                        out.push_str(&format_control(c));
                    }
                }
                out.push('\n');
            }
            Err(_) => {
                out.push_str(&format!("{}: (requires config to inspect)\n\n", type_name));
            }
        }
    }
    Ok(out.trim_end().to_string())
}

// ---------------------------------------------------------------------------
// Help
// ---------------------------------------------------------------------------

fn help_text() -> String {
    "\
Fugue REPL — interactive modular synthesis

Lifecycle:
  new [title]                        Create a new invention (DAC only)
  load <path>                        Load invention from JSON file
  load-json <json>                   Load invention from inline JSON
  save <path>                        Save invention to JSON file
  stop                               Stop playback
  status                             Show running state

Modules:
  add <id> <type> [config_json]      Add a module
  remove <id>                        Remove a module
  modules                            List all modules

Connections:
  connect <from> <port> <to> <port>  Wire two modules
  disconnect <from> <port> <to> <port>  Remove a connection
  connections                        List all connections

Controls:
  set <module> <key> <value>         Set a control value
  get <module> <key>                 Get a control value
  controls [module]                  List controls

Discovery:
  types                              Describe all module types

  help                               Show this help
  quit / exit                        Stop and exit"
        .to_string()
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

fn main() {
    let sample_rate = default_sample_rate().unwrap_or(44100);
    let mut repl = FugueRepl::new(sample_rate);

    let mut rl = DefaultEditor::new().expect("Failed to initialize readline");

    let history_path = dirs_history_path();
    if let Some(ref path) = history_path {
        let _ = rl.load_history(path);
    }

    // Dev auto-restore: if FUGUE_DEV_STATE is set and the file exists, load it
    let dev_state_path = std::env::var("FUGUE_DEV_STATE").ok();
    if let Some(ref path) = dev_state_path {
        if std::path::Path::new(path).exists() {
            match Invention::from_file(path) {
                Ok(invention) => match start_invention(&mut repl, invention) {
                    Ok(msg) => println!("[dev] Auto-restored: {}", msg),
                    Err(e) => eprintln!("[dev] Restore failed: {}", e),
                },
                Err(e) => eprintln!("[dev] Failed to read state file: {}", e),
            }
        }
    }

    if let Some(path) = parse_invention_flag() {
        match Invention::from_file(&path) {
            Ok(invention) => match start_invention(&mut repl, invention) {
                Ok(msg) => println!("{}", msg),
                Err(e) => eprintln!("Error loading invention: {}", e),
            },
            Err(e) => eprintln!("Error reading invention file '{}': {}", path, e),
        }
    }

    println!("Fugue REPL (type 'help' for commands, 'quit' to exit)");

    loop {
        match rl.readline("fugue> ") {
            Ok(line) => {
                let line = line.trim().to_string();
                if line.is_empty() {
                    continue;
                }
                let _ = rl.add_history_entry(&line);

                match execute(&mut repl, &line) {
                    Ok(output) => {
                        if !output.is_empty() {
                            println!("{}", output);
                        }
                    }
                    Err(err) => {
                        eprintln!("Error: {}", err);
                    }
                }
            }
            Err(ReadlineError::Interrupted) => {
                println!("^C (use 'quit' to exit)");
            }
            Err(ReadlineError::Eof) => {
                dev_save_state(&repl);
                repl.stop_current();
                break;
            }
            Err(err) => {
                eprintln!("Readline error: {}", err);
                break;
            }
        }
    }

    if let Some(ref path) = history_path {
        let _ = rl.save_history(path);
    }
}

fn parse_invention_flag() -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    let mut iter = args.iter().skip(1);
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--invention" | "-i" => return iter.next().cloned(),
            _ => {}
        }
    }
    None
}

fn dev_save_state(repl: &FugueRepl) {
    if let Ok(path) = std::env::var("FUGUE_DEV_STATE") {
        if !repl.modules.is_empty() {
            let invention = repl.to_invention();
            if let Ok(json) = invention.to_json() {
                match std::fs::write(&path, &json) {
                    Ok(_) => eprintln!("[dev] State saved to {}", path),
                    Err(e) => eprintln!("[dev] Failed to save state: {}", e),
                }
            }
        }
    }
}

fn dirs_history_path() -> Option<String> {
    dirs_home().map(|h| format!("{}/.fugue_history", h))
}

fn dirs_home() -> Option<String> {
    std::env::var("HOME")
        .ok()
        .or_else(|| std::env::var("USERPROFILE").ok())
}
