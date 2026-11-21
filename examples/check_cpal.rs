use cpal::traits::{DeviceTrait, HostTrait};

fn main() {
    let host = cpal::default_host();
    println!("Host ID: {:?}", host.id());
    println!("=== INPUT DEVICES ===");
    if let Ok(devs) = host.input_devices() {
        for d in devs {
            println!("Input: {:?}", d.name());
        }
    }
    println!("=== OUTPUT DEVICES ===");
    if let Ok(devs) = host.output_devices() {
        for d in devs {
            println!("Output: {:?}", d.name());
        }
    }
    println!("Default input: {:?}", host.default_input_device().and_then(|d| d.name().ok()));
    println!("Default output: {:?}", host.default_output_device().and_then(|d| d.name().ok()));
}
