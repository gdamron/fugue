use super::*;

pub(crate) fn module_ports(
    modules: &IndexMap<String, ModuleInstance>,
) -> IndexMap<String, ModulePorts> {
    modules
        .iter()
        .map(|(id, module)| {
            (
                id.clone(),
                ModulePorts {
                    inputs: module
                        .module()
                        .inputs()
                        .iter()
                        .map(|port| (*port).to_string())
                        .collect(),
                    outputs: module
                        .module()
                        .outputs()
                        .iter()
                        .map(|port| (*port).to_string())
                        .collect(),
                },
            )
        })
        .collect()
}

/// Validates that a module has the specified output port.
pub(crate) fn validate_output_port(
    module: &ModuleInstance,
    port: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let m = module.module();
    if !m.outputs().contains(&port) {
        return Err(format!(
            "Module '{}' does not have output port '{}'. Available: {:?}",
            m.name(),
            port,
            m.outputs()
        )
        .into());
    }
    Ok(())
}

/// Validates that a module has the specified input port.
pub(crate) fn validate_input_port(
    module: &ModuleInstance,
    port: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let m = module.module();
    if !m.inputs().contains(&port) {
        return Err(format!(
            "Module '{}' does not have input port '{}'. Available: {:?}",
            m.name(),
            port,
            m.inputs()
        )
        .into());
    }
    Ok(())
}
