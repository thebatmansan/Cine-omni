//! Reports the real GPU adapter, so "GPU verified" is never an assumption.
fn main() {
    match cine_gpu::device::GpuContext::new() {
        Ok(ctx) => {
            let info = ctx.adapter.get_info();
            println!("ADAPTER OK");
            println!("  name    {}", info.name);
            println!("  backend {:?}", info.backend);
            println!("  type    {:?}", info.device_type);
            println!("  driver  {} {}", info.driver, info.driver_info);
        }
        Err(e) => println!("NO ADAPTER: {e}"),
    }
}
