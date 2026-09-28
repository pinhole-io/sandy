pub fn run_firecracker_agent() -> Result<(), Box<dyn std::error::Error>> {
    crate::service::firecracker::agent::run()
}
