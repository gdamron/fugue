wit_bindgen::generate!({
    path: "../../../wit",
    world: "module",
});

use std::cell::RefCell;

struct TestModule;

struct State {
    sample_rate: f32,
    frequency: f32,
    phase: f32,
    output: [f32; 1024],
}

impl Default for State {
    fn default() -> Self {
        Self {
            sample_rate: 48_000.0,
            frequency: 440.0,
            phase: 0.0,
            output: [0.0; 1024],
        }
    }
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

impl Guest for TestModule {
    fn init(sample_rate: u32, _config_json: String) -> Result<(), String> {
        STATE.with_borrow_mut(|state| {
            state.sample_rate = sample_rate as f32;
            state.frequency = 440.0;
            state.phase = 0.0;
            state.output.fill(0.0);
        });
        Ok(())
    }

    fn name() -> String {
        "FixtureOscillator".to_string()
    }

    fn inputs() -> Vec<String> {
        vec!["frequency".to_string()]
    }

    fn outputs() -> Vec<String> {
        vec!["audio".to_string()]
    }

    fn set_input(port: String, value: f32) -> Result<(), String> {
        if port != "frequency" {
            return Err(format!("unknown input: {port}"));
        }
        STATE.with_borrow_mut(|state| state.frequency = value);
        Ok(())
    }

    fn set_input_connected(_index: u32, _connected: bool) {}

    fn set_input_block(index: u32, values: Vec<f32>) -> Result<(), String> {
        if index != 0 {
            return Err(format!("unknown input index: {index}"));
        }
        if let Some(value) = values.first() {
            STATE.with_borrow_mut(|state| state.frequency = *value);
        }
        Ok(())
    }

    fn process(frames: u32) -> bool {
        STATE.with_borrow_mut(|state| {
            let frames = (frames as usize).min(state.output.len());
            for i in 0..frames {
                state.output[i] = state.phase;
                state.phase += state.frequency / state.sample_rate;
                state.phase %= 1.0;
            }
        });
        true
    }

    fn process_output_block(frames: u32) -> Result<Vec<f32>, String> {
        Self::process(frames);
        Self::output_block(0)
    }

    fn output_block(index: u32) -> Result<Vec<f32>, String> {
        if index != 0 {
            return Err(format!("unknown output index: {index}"));
        }
        STATE.with_borrow(|state| Ok(state.output.to_vec()))
    }

    fn get_output(port: String) -> Result<f32, String> {
        if port != "audio" {
            return Err(format!("unknown output: {port}"));
        }
        STATE.with_borrow(|state| Ok(state.output[0]))
    }

    fn set_control(key: String, value: f32) -> Result<(), String> {
        if key != "frequency" {
            return Err(format!("unknown control: {key}"));
        }
        STATE.with_borrow_mut(|state| state.frequency = value);
        Ok(())
    }
}

export!(TestModule);
