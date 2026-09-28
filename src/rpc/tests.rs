use super::*;
use crate::{Invention, ModuleSpec};

mod discovery;
mod error;
mod event;
mod package;
mod recovery;
mod request;
mod response;
mod snapshot;
mod spectrogram;

fn test_invention() -> Invention {
    Invention {
        version: "1.0.0".to_string(),
        title: Some("rpc-test".to_string()),
        description: None,
        developments: Vec::new(),
        assets: std::collections::BTreeMap::new(),
        modules: vec![ModuleSpec {
            id: "dac".to_string(),
            module_type: "dac".to_string(),
            config: serde_json::Value::Null,
        }],
        connections: Vec::new(),
        inputs: Vec::new(),
        outputs: Vec::new(),
        controls: Vec::new(),
        source_path: None,
    }
}

fn revision(session: &str, revision: u64) -> RuntimeRevision {
    RuntimeRevision {
        session_id: session.to_string(),
        revision,
    }
}

#[test]
fn schema_version_rejects_incompatible_clients() {
    let error = validate_schema_version(RPC_SCHEMA_VERSION + 1).unwrap_err();
    assert_eq!(error.code, RpcErrorCode::IncompatibleSchemaVersion);
    assert!(error.message.contains("incompatible RPC schema version"));
}

#[cfg(feature = "rpc-schema")]
#[test]
fn runtime_rpc_schema_generates() {
    let schema = schema::runtime_rpc_schema();
    let json = serde_json::to_value(schema).unwrap();
    assert!(json.is_object());
}
