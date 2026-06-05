use super::*;

struct TestModule {
    input_a: [f32; MAX_BLOCK],
    input_b: [f32; MAX_BLOCK],
    sum: [f32; MAX_BLOCK],
    product: [f32; MAX_BLOCK],
}

impl TestModule {
    fn new() -> Self {
        Self {
            input_a: [0.0; MAX_BLOCK],
            input_b: [0.0; MAX_BLOCK],
            sum: [0.0; MAX_BLOCK],
            product: [0.0; MAX_BLOCK],
        }
    }
}

impl Module for TestModule {
    fn name(&self) -> &str {
        "TestModule"
    }

    fn process(&mut self, frames: usize) -> bool {
        for i in 0..frames {
            self.sum[i] = self.input_a[i] + self.input_b[i];
            self.product[i] = self.input_a[i] * self.input_b[i];
        }
        true
    }

    fn inputs(&self) -> &[&str] {
        &["a", "b"]
    }

    fn outputs(&self) -> &[&str] {
        &["sum", "product"]
    }

    fn input_block_mut(&mut self, index: usize) -> &mut [f32] {
        match index {
            0 => &mut self.input_a,
            _ => &mut self.input_b,
        }
    }

    fn output_block(&self, index: usize) -> &[f32] {
        match index {
            0 => &self.sum,
            _ => &self.product,
        }
    }

    fn set_input(&mut self, port: &str, value: f32) -> Result<(), String> {
        match port {
            "a" => {
                self.input_a.fill(value);
                Ok(())
            }
            "b" => {
                self.input_b.fill(value);
                Ok(())
            }
            _ => Err(format!("Unknown input port: {}", port)),
        }
    }

    fn get_output(&self, port: &str) -> Result<f32, String> {
        match port {
            "sum" => Ok(self.sum[0]),
            "product" => Ok(self.product[0]),
            _ => Err(format!("Unknown output port: {}", port)),
        }
    }
}

#[test]
fn test_module() {
    let mut module = TestModule::new();

    // Test setting inputs
    assert!(module.set_input("a", 3.0).is_ok());
    assert!(module.set_input("b", 4.0).is_ok());
    assert!(module.set_input("c", 5.0).is_err());

    // Process a one-frame block and read outputs.
    module.process(1);
    assert_eq!(module.get_output("sum").unwrap(), 7.0);
    assert_eq!(module.get_output("product").unwrap(), 12.0);
    assert!(module.get_output("invalid").is_err());

    // Test output after input change
    module.set_input("a", 5.0).unwrap();
    module.set_input("b", 6.0).unwrap();
    module.process(1);
    assert_eq!(module.get_output("sum").unwrap(), 11.0);
    assert_eq!(module.get_output("product").unwrap(), 30.0);
    assert!(module.get_output("invalid").is_err());
}

#[test]
fn test_validate_port() {
    let ports = &["audio", "cv", "gate"];

    assert!(validate_port("audio", ports, "input").is_ok());
    assert!(validate_port("cv", ports, "input").is_ok());
    assert!(validate_port("invalid", ports, "input").is_err());
}
