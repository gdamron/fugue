//! Declared controls through their module's surface: a control its module
//! reads, and payload indices past a read-only count.

use std::sync::Arc;

use super::*;
use crate::control_request::ControlCells;
use crate::invention::declared::DeclaredSurface;
use crate::ControlSurface;

/// The names a picker's `pick` answers to, attached to its surface.
struct Names(&'static [&'static str]);

const PICK: ValueReader = ValueReader { read: read_pick };

fn read_pick(context: &ControlContext<'_>, value: &ControlValue) -> Result<RtValue, String> {
    let Names(names) = context.module::<Names>()?;
    let name = value.as_string()?;
    let position = names.iter().position(|known| *known == name.trim());
    position
        .map(|position| RtValue::I32(position as i32))
        .ok_or_else(|| format!("No choice named '{}'", name.trim()))
}

const PICKER: &[ControlDecl] = &[ControlDecl::new(
    "pick",
    DeclKind::Integer { min: 0, max: 1 },
    RtValue::I32(0),
    "Pick",
)
.read_by(PICK)];

static PICKER_TABLE: ControlTable = ControlTable::of(PICKER);

fn picker(context: bool) -> (DeclaredSurface, Arc<ControlCells>) {
    let cells = Arc::new(ControlCells::new([RtValue::I32(0)]));
    let surface = DeclaredSurface::new(PICKER_TABLE.clone(), cells.clone());
    match context {
        true => (
            surface.with_context(Arc::new(Names(&["low", "high"]))),
            cells,
        ),
        false => (surface, cells),
    }
}

#[test]
fn a_control_its_module_reads_is_read_by_its_surface_and_lists_as_text() {
    let (surface, cells) = picker(true);
    let surfaces = Default::default();
    assert_eq!(
        surface.validate_control("pick", &"high".into(), &surfaces),
        Ok(())
    );
    assert_eq!(
        surface.validate_control("pick", &"mid".into(), &surfaces),
        Err("No choice named 'mid'".into())
    );
    surface.set_control("pick", " high ".into()).unwrap();
    assert_eq!(cells.load(ControlIndex(0)), Some(RtValue::I32(1)));

    assert_eq!(
        surface.get_control("pick"),
        Ok("".into()),
        "reads back as text"
    );
    let listed = &surface.controls()[0];
    assert_eq!(listed.kind, ControlKind::String { options: None });
    assert!(
        surface.automation("pick").is_none(),
        "no schedule writes it"
    );
    assert_eq!(
        PICKER_TABLE.coerce(ControlIndex(0), &ControlValue::Number(1.0)),
        Err("Control 'pick' is read by its module's own surface".into()),
        "nor does its kind's rule read it"
    );
}

#[test]
fn a_surface_without_its_modules_context_refuses_a_read_control() {
    let (surface, _) = picker(false);
    assert_eq!(
        surface.set_control("pick", "high".into()),
        Err("this control is written only through its module's own surface".into())
    );
}

/// Whatever was written, kept as its text.
const CLIP: PayloadCodec = PayloadCodec { prepare: clip };

fn clip(_: &ControlContext<'_>, value: &ControlValue) -> Prepared {
    let text = value.as_string()?.to_string();
    Ok((crate::payload::Payload::new(text.clone()), text.into()))
}

/// Two clip slots listed below a fixed `slots` of 1.
const CLIPS: &[ControlDecl] = &[
    ControlDecl::new(
        "slots",
        DeclKind::Number { min: 0.0, max: 2.0 },
        RtValue::F32(1.0),
        "Slots",
    )
    .telemetry(),
    ControlDecl::payload("clip", CLIP, "A slot's clip")
        .indexed(2)
        .listed_below("slots"),
];

static CLIPS_TABLE: ControlTable = ControlTable::of(CLIPS);

#[test]
fn a_payload_index_past_a_read_only_count_is_no_control() {
    let cells = Arc::new(ControlCells::new(CLIPS.iter().map(|decl| decl.default)));
    let surface = DeclaredSurface::new(CLIPS_TABLE.clone(), cells);
    let gone = "Unknown control: clip.1: slots is 1, so only 'clip.0' exists".to_string();

    surface.show_payload("clip.0", "kick".into());
    surface.show_payload("clip.1", "snare".into());
    assert_eq!(surface.get_control("clip.0"), Ok("kick".into()));
    assert_eq!(
        surface.get_control("clip.1"),
        Err(gone.clone()),
        "nothing shown past it"
    );

    assert!(surface.is_payload("clip.0"));
    assert!(!surface.is_payload("clip.1"));
    let surfaces = Default::default();
    let write: ControlValue = "hat".into();
    assert_eq!(
        surface.validate_control("clip.1", &write, &surfaces),
        Err(gone.clone())
    );
    assert_eq!(
        surface.set_control("clip.1", write.clone()),
        Err(gone.clone())
    );
    let payload = crate::payload::Payload::new(String::from("hat"));
    assert_eq!(
        surface.write_payload("clip.1", payload, write.clone()),
        Err(gone.clone())
    );

    surface.set_control("clip.0", write).unwrap();
    assert_eq!(surface.get_control("clip.0"), Ok("hat".into()));
}
