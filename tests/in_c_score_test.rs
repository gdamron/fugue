use fugue::ModuleRegistry;
use serde_json::Value;

fn score() -> Value {
    serde_json::from_str(include_str!("../examples/in_c/score.json")).unwrap()
}

#[test]
fn in_c_score_is_well_formed() {
    let score = score();

    assert_eq!(score["base_note_hint"], 60);
    assert_eq!(score["rhythm_grid"], "32nd_note");

    let cells = score["cells"].as_array().expect("cells should be an array");
    assert_eq!(cells.len(), 53);

    for (cell_index, cell) in cells.iter().enumerate() {
        let steps = cell.as_array().expect("each cell should be an array");
        assert!(
            !steps.is_empty(),
            "cell {} should contain at least one step",
            cell_index + 1
        );
        assert!(
            steps.len() <= 256,
            "cell {} has too many steps: {}",
            cell_index + 1,
            steps.len()
        );

        let mut has_active_note = false;
        for step in steps {
            let object = step.as_object().expect("each step should be an object");
            assert!(
                object
                    .keys()
                    .all(|key| key == "note" || key == "gate" || key == "held"),
                "unsupported step field in cell {}: {:?}",
                cell_index + 1,
                object
            );

            if let Some(held) = object.get("held") {
                assert_eq!(held, true, "held should only be present when true");
                assert!(
                    object.len() == 1,
                    "held steps should not include note or gate in cell {}",
                    cell_index + 1
                );
                assert!(
                    has_active_note,
                    "held step without active note in cell {}",
                    cell_index + 1
                );
                continue;
            }

            let note = object
                .get("note")
                .expect("step should include note or held");
            if note.is_null() {
                has_active_note = false;
            } else {
                assert!(
                    note.as_i64().is_some(),
                    "note should be an integer or null in cell {}",
                    cell_index + 1
                );
                has_active_note = true;
            }

            if let Some(gate) = object.get("gate") {
                let gate = gate.as_f64().expect("gate should be numeric");
                assert!(
                    (0.0..=1.0).contains(&gate),
                    "gate should be within 0.0..=1.0"
                );
            }
        }
    }
}

#[test]
fn in_c_score_builds_a_cell_sequencer() {
    let score = score();
    let config = serde_json::json!({
        "base_note": score["base_note_hint"].clone(),
        "steps": 256,
        "sequences": score["cells"].clone()
    });

    let registry = ModuleRegistry::default();
    registry.build("cell_sequencer", 44_100, &config).unwrap();
}

#[test]
fn in_c_cell_one_regression() {
    let score = score();
    let cells = score["cells"].as_array().unwrap();
    let cell_one = cells[0].as_array().unwrap();

    assert_eq!(cell_one.len(), 24);
    for (index, note) in [(0, 0), (1, 4), (8, 0), (9, 4), (16, 0), (17, 4)] {
        assert_eq!(cell_one[index]["note"], note);
    }
    for index in [
        2, 3, 4, 5, 6, 7, 10, 11, 12, 13, 14, 15, 18, 19, 20, 21, 22, 23,
    ] {
        assert_eq!(cell_one[index]["held"], true);
    }
}

#[test]
fn in_c_cell_two_regression() {
    let score = score();
    let cells = score["cells"].as_array().unwrap();
    let cell_two = cells[1].as_array().unwrap();

    assert_eq!(cell_two.len(), 16);
    for (index, note) in [(0, 0), (1, 4), (4, 5), (8, 4)] {
        assert_eq!(cell_two[index]["note"], note);
    }
    for index in [2, 3, 5, 6, 7, 9, 10, 11, 12, 13, 14, 15] {
        assert_eq!(cell_two[index]["held"], true);
    }
}
