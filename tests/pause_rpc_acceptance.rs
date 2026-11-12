//! The RPC commands that pause and resume a loaded piece: their wire shape
//! and how the daemon classifies them.

use fugue::rpc::RpcCommand;
use serde_json::json;

const COMMANDS: [(RpcCommand, &str); 2] = [
    (RpcCommand::PauseInvention, "pause_invention"),
    (RpcCommand::ResumeInvention, "resume_invention"),
];

/// Each is a unit command: `{"command":"pause_invention"}` on the wire, in
/// both directions.
#[test]
fn pause_and_resume_have_a_unit_wire_shape() {
    for (command, name) in COMMANDS {
        let wire = json!({ "command": name });
        assert_eq!(serde_json::to_value(&command).unwrap(), wire);
        let parsed: RpcCommand = serde_json::from_value(wire).unwrap();
        assert_eq!(parsed, command);
        let parsed: RpcCommand =
            serde_json::from_str(&format!(r#"{{"command":"{name}"}}"#)).unwrap();
        assert_eq!(parsed, command);
    }
}

/// Both answer with a snapshot and leave the revision alone: they change
/// playback, not the authored piece.
#[test]
fn pause_and_resume_reply_with_a_snapshot_and_keep_the_revision() {
    for (command, name) in COMMANDS {
        assert!(command.replies_with_snapshot(), "{name}");
        assert!(!command.advances_revision(), "{name}");
    }
}
